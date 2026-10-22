//! Shared execution envelope for all external model/judgment calls (R4.T0).
//!
//! One shell for [`LlmClient`](crate::llm::LlmClient) (chat) and
//! [`JudgeClient`](crate::llm::judge::JudgeClient) (judgment): retry with
//! exponential backoff, deadline timeout, sanitized error classification,
//! metering rows, and the egress gate hook. Backends supply a call factory
//! that returns the decoded value plus its [`CallMeta`]; the envelope owns
//! the retry loop so no backend re-implements it.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use thiserror::Error;

/// Capability family of the outbound call. The egress gate is per-capability:
/// `Judge` is off by default, `Chat` keeps the R1 status quo, `Embedding`
/// never leaves the process (local fastembed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Embedding,
    Chat,
    Judge,
}

/// Per-call metadata recorded to the [`MeteringSink`].
#[derive(Debug, Clone)]
pub struct CallMeta {
    /// Actual model (backend echo; config value + note when unreported).
    pub model: String,
    /// Usage as reported by the service; 0 when unreported.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Candidate scale of the call (items / passages / candidates).
    pub candidate_scale: usize,
    pub from_cache: bool,
}

/// One aggregated metering row (the `snapshot` output for cost reports).
#[derive(Debug, Clone)]
pub struct MeteringRow {
    pub op: Op,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub candidate_scale: usize,
    pub from_cache: bool,
}

/// Sanitized provider error: fixed code + op. Never carries an upstream
/// response body — secret values and endpoint internals stay out of
/// client-visible errors and logs.
#[derive(Debug, Clone, Error)]
pub enum ProviderError {
    #[error("deadline exceeded for {op:?}")]
    Timeout { op: Op },
    #[error("rate limited for {op:?}")]
    RateLimited {
        op: Op,
        retry_after: Option<Duration>,
    },
    #[error("backend unavailable for {op:?}")]
    Unavailable { op: Op },
    #[error("authentication failed for {op:?}")]
    AuthFailed { op: Op },
    #[error("invalid response for {op:?}")]
    InvalidResponse { op: Op },
    #[error("egress policy denied")]
    PolicyDenied,
    #[error("disabled client invoked")]
    Disabled,
}

/// Raw classification of an outbound call failure, produced by the backend's
/// call factory. The envelope maps it to a sanitized [`ProviderError`] per
/// the failure-mapping table (retryable: transport, 408/409/429/5xx).
#[derive(Debug)]
pub enum RawCallError {
    /// Non-2xx HTTP response; `retry_after` parses the Retry-After header.
    Http {
        status: u16,
        retry_after: Option<Duration>,
    },
    /// DNS/TLS/connect failure.
    Transport,
    /// Malformed response / schema mismatch (fix the call, not the retry).
    InvalidResponse,
}

/// Retry/backoff/timeout policy. Defaults align with the jev endpoint
/// (60s deadline); chat keeps its reference-parity values.
#[derive(Debug, Clone)]
pub struct Policy {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub timeout: Duration,
}

/// Retry-After header as a duration (seconds form only).
pub(crate) fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value.parse::<u64>().ok().map(Duration::from_secs)
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(10),
            timeout: Duration::from_secs(60),
        }
    }
}

/// High-confidence credential anchors (§外发门规则). Compiled once.
static CREDENTIAL_PATTERNS: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
    const RAW: &[&str] = &[
        r"sk-[A-Za-z0-9]{20,}",
        r"ghp_[A-Za-z0-9]{36}",
        r"xox[baprs]-[A-Za-z0-9-]+",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r#"(password|passwd|secret|api[_-]?key)\s*[:=]\s*["']?[^\s"']{8,}"#,
    ];
    RAW.iter()
        .map(|p| regex::Regex::new(p).expect("credential anchor is a valid regex"))
        .collect()
});

/// The egress gate: per-capability opt-in plus credential precheck.
///
/// The judge capability is off until the workspace opts in (`[judge] enabled`
/// in config); until then judge calls are denied before any network I/O
/// (zero-egress, AC1). The credential precheck blocks a whole batch when a
/// high-confidence credential pattern is present in the outbound payload.
/// Precheck results are never logged and secret values never enter any
/// artifact.
#[derive(Debug, Clone, Copy)]
pub struct EgressGate {
    judge_enabled: bool,
}

impl EgressGate {
    pub fn new(judge_enabled: bool) -> Self {
        Self { judge_enabled }
    }

    /// Op-level policy: judge calls are denied until opted in.
    pub fn check(&self, op: Op) -> Result<(), ProviderError> {
        if op == Op::Judge && !self.judge_enabled {
            Err(ProviderError::PolicyDenied)
        } else {
            Ok(())
        }
    }

    /// Credential precheck over the outbound payload. High-confidence
    /// credential patterns block the whole batch; ordinary code, paths and
    /// business content pass within the authorized scope.
    pub fn precheck(&self, payload: &str) -> Result<(), ProviderError> {
        if CREDENTIAL_PATTERNS.iter().any(|p| p.is_match(payload)) {
            Err(ProviderError::PolicyDenied)
        } else {
            Ok(())
        }
    }
}

/// Metering sink: per-call rows for the cost report.
pub trait MeteringSink: Send + Sync {
    fn record(&self, op: Op, meta: &CallMeta);
    fn snapshot(&self) -> Vec<MeteringRow>;
}

/// Aggregated counters per (op, model).
#[derive(Debug, Default, Clone)]
struct ModelCounters {
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    candidate_scale: usize,
    cache_hits: u64,
}

/// Default sink: a structured `tracing::info!` event per call plus
/// in-process counters aggregated per (op, model) for `snapshot`.
pub struct DefaultMeteringSink {
    counters: Mutex<std::collections::HashMap<(Op, String), ModelCounters>>,
}

impl DefaultMeteringSink {
    pub fn new() -> Self {
        Self {
            counters: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for DefaultMeteringSink {
    fn default() -> Self {
        Self::new()
    }
}

impl MeteringSink for DefaultMeteringSink {
    fn record(&self, op: Op, meta: &CallMeta) {
        tracing::info!(
            op = ?op,
            model = %meta.model,
            input_tokens = meta.input_tokens,
            output_tokens = meta.output_tokens,
            candidate_scale = meta.candidate_scale,
            from_cache = meta.from_cache,
            "model call metered"
        );
        let mut counters = self
            .counters
            .lock()
            .expect("metering counters lock poisoned");
        let entry = counters.entry((op, meta.model.clone())).or_default();
        entry.calls += 1;
        entry.input_tokens += meta.input_tokens;
        entry.output_tokens += meta.output_tokens;
        entry.candidate_scale += meta.candidate_scale;
        entry.cache_hits += u64::from(meta.from_cache);
    }

    fn snapshot(&self) -> Vec<MeteringRow> {
        let counters = self
            .counters
            .lock()
            .expect("metering counters lock poisoned");
        let mut rows: Vec<MeteringRow> = counters
            .iter()
            .map(|((op, model), c)| MeteringRow {
                op: *op,
                model: model.clone(),
                input_tokens: c.input_tokens,
                output_tokens: c.output_tokens,
                candidate_scale: c.candidate_scale,
                from_cache: c.cache_hits > 0,
            })
            .collect();
        rows.sort_by_key(|r| (r.op as u8, r.model.clone()));
        rows
    }
}

/// Process-wide counter seeding per-call jitter (no `rand` dependency; the
/// mix varies per call, which is all backoff jitter needs).
static JITTER_SEED: AtomicU64 = AtomicU64::new(0);

/// Exponential backoff capped at `max_backoff`, with per-call jitter in
/// [0.5, 1.0)× so concurrent callers do not retry in lockstep.
fn backoff_for(policy: &Policy, attempt: u32) -> Duration {
    let exponent = (attempt - 1).min(16);
    let base = policy.initial_backoff.saturating_mul(1u32 << exponent);
    let capped = base.min(policy.max_backoff).max(policy.initial_backoff);
    let n = JITTER_SEED.fetch_add(1, Ordering::Relaxed);
    let mix = (n.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) as u32;
    let fraction = (mix % 1000) as f64 / 1000.0;
    let jittered = capped.mul_f64(0.5 + fraction * 0.5);
    jittered.max(Duration::from_millis(1))
}

/// Execute one outbound call through the envelope:
///
/// 1. `gate.check(op)` — judge not opted in → `PolicyDenied` (zero calls).
/// 2. Attempt loop: `tokio::time::timeout(policy.timeout, call())`;
///    transport / 408/409/429/5xx → exponential backoff retry (429 honors
///    Retry-After); 401/403 → `AuthFailed`; other 4xx and malformed
///    responses → `InvalidResponse`; deadline → `Timeout`.
/// 3. `sink.record(op, &meta)` on success.
pub async fn execute<T, F, Fut>(
    op: Op,
    policy: &Policy,
    gate: &EgressGate,
    sink: &dyn MeteringSink,
    mut call: F,
) -> Result<(T, CallMeta), ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(T, CallMeta), RawCallError>>,
{
    gate.check(op)?;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let outcome = tokio::time::timeout(policy.timeout, call()).await;
        match outcome {
            Err(_elapsed) => {
                return Err(ProviderError::Timeout { op });
            }
            Ok(Err(RawCallError::Http {
                status,
                retry_after,
            })) => {
                let retryable = matches!(status, 408 | 409 | 429) || status >= 500;
                if retryable && attempt < policy.max_attempts {
                    let backoff = match status {
                        429 => retry_after.unwrap_or_else(|| backoff_for(policy, attempt)),
                        _ => backoff_for(policy, attempt),
                    };
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                return Err(match status {
                    401 | 403 => ProviderError::AuthFailed { op },
                    429 => ProviderError::RateLimited { op, retry_after },
                    408 | 409 => ProviderError::Unavailable { op },
                    _ if status >= 500 => ProviderError::Unavailable { op },
                    _ => ProviderError::InvalidResponse { op },
                });
            }
            Ok(Err(RawCallError::Transport)) => {
                if attempt < policy.max_attempts {
                    tokio::time::sleep(backoff_for(policy, attempt)).await;
                    continue;
                }
                return Err(ProviderError::Unavailable { op });
            }
            Ok(Err(RawCallError::InvalidResponse)) => {
                return Err(ProviderError::InvalidResponse { op });
            }
            Ok(Ok((value, meta))) => {
                sink.record(op, &meta);
                return Ok((value, meta));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(attempts: u32) -> Policy {
        Policy {
            max_attempts: attempts,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            timeout: Duration::from_secs(5),
        }
    }

    fn meta(model: &str) -> CallMeta {
        CallMeta {
            model: model.into(),
            input_tokens: 0,
            output_tokens: 0,
            candidate_scale: 0,
            from_cache: false,
        }
    }

    struct CountingSink {
        calls: AtomicU64,
    }

    impl MeteringSink for CountingSink {
        fn record(&self, _op: Op, _meta: &CallMeta) {
            self.calls.fetch_add(1, Ordering::Relaxed);
        }
        fn snapshot(&self) -> Vec<MeteringRow> {
            vec![]
        }
    }

    #[tokio::test]
    async fn gate_denies_judge_until_opted_in() {
        let gate = EgressGate::new(false);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let mut invoked = false;
        let result: Result<(String, CallMeta), ProviderError> =
            execute(Op::Judge, &policy(3), &gate, &sink, || {
                invoked = true;
                async { Ok(("value".to_string(), meta("m"))) }
            })
            .await;
        assert!(matches!(result, Err(ProviderError::PolicyDenied)));
        assert!(!invoked, "call factory must not run when denied");
        assert_eq!(sink.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn chat_passes_the_gate_without_opt_in() {
        let gate = EgressGate::new(false);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let result: Result<(String, CallMeta), ProviderError> =
            execute(Op::Chat, &policy(3), &gate, &sink, || async {
                Ok(("value".to_string(), meta("m")))
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(sink.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn retries_transport_until_success() {
        let gate = EgressGate::new(true);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let mut attempts = 0u32;
        let result: Result<(u32, CallMeta), ProviderError> =
            execute(Op::Judge, &policy(3), &gate, &sink, || {
                attempts += 1;
                let failed = attempts < 3;
                async move {
                    if failed {
                        Err(RawCallError::Transport)
                    } else {
                        Ok((attempts, meta("m")))
                    }
                }
            })
            .await;
        assert_eq!(result.expect("third attempt succeeds").0, 3);
        assert_eq!(attempts, 3);
    }

    #[tokio::test]
    async fn auth_failure_is_not_retried() {
        let gate = EgressGate::new(true);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let mut attempts = 0u32;
        let result: Result<(String, CallMeta), ProviderError> =
            execute(Op::Judge, &policy(3), &gate, &sink, || {
                attempts += 1;
                async {
                    Err(RawCallError::Http {
                        status: 401,
                        retry_after: None,
                    })
                }
            })
            .await;
        assert!(matches!(result, Err(ProviderError::AuthFailed { .. })));
        assert_eq!(attempts, 1, "401 must not be retried");
    }

    #[tokio::test]
    async fn invalid_response_is_not_retried() {
        let gate = EgressGate::new(true);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let mut attempts = 0u32;
        let result: Result<(String, CallMeta), ProviderError> =
            execute(Op::Judge, &policy(3), &gate, &sink, || {
                attempts += 1;
                async { Err(RawCallError::InvalidResponse) }
            })
            .await;
        assert!(matches!(result, Err(ProviderError::InvalidResponse { .. })));
        assert_eq!(attempts, 1, "invalid responses must not be retried");
    }

    #[tokio::test]
    async fn rate_limited_surfaces_retry_after() {
        let gate = EgressGate::new(true);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let retry_after = Duration::from_secs(7);
        let result: Result<(String, CallMeta), ProviderError> =
            execute(Op::Judge, &policy(1), &gate, &sink, || async {
                Err(RawCallError::Http {
                    status: 429,
                    retry_after: Some(retry_after),
                })
            })
            .await;
        match result {
            Err(ProviderError::RateLimited {
                op,
                retry_after: ra,
            }) => {
                assert_eq!(op, Op::Judge);
                assert_eq!(ra, Some(retry_after));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deadline_timeout_maps_to_timeout_error() {
        let gate = EgressGate::new(true);
        let sink = CountingSink {
            calls: AtomicU64::new(0),
        };
        let result: Result<(String, CallMeta), ProviderError> = execute(
            Op::Judge,
            &Policy {
                max_attempts: 1,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(1),
                timeout: Duration::from_millis(10),
            },
            &gate,
            &sink,
            || async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(("late".to_string(), meta("m")))
            },
        )
        .await;
        assert!(matches!(result, Err(ProviderError::Timeout { .. })));
    }

    #[test]
    fn credential_precheck_blocks_high_confidence_patterns() {
        let gate = EgressGate::new(true);
        assert!(gate
            .precheck("sk-abcdefghijklmnopqrstuvwxyz123456")
            .is_err());
        assert!(gate
            .precheck("token ghp_abcdefghijklmnopqrstuvwxyz0123456789")
            .is_err());
        assert!(gate.precheck("xoxb-1234567890-abcdef").is_err());
        assert!(gate.precheck("-----BEGIN RSA PRIVATE KEY-----").is_err());
        assert!(gate.precheck("api_key = \"supersecretvalue\"").is_err());
        // ordinary code / paths / business content passes
        assert!(gate.precheck("SELECT * FROM memory WHERE id = 1;").is_ok());
        assert!(gate.precheck("/home/user/project/src/main.rs").is_ok());
        assert!(gate.precheck("ACME acquired Beta Labs for $10M.").is_ok());
    }

    #[test]
    fn default_sink_snapshots_aggregated_rows() {
        let sink = DefaultMeteringSink::new();
        let m = CallMeta {
            model: "judge-1".into(),
            input_tokens: 10,
            output_tokens: 5,
            candidate_scale: 3,
            from_cache: false,
        };
        sink.record(Op::Judge, &m);
        sink.record(Op::Judge, &m);
        let rows = sink.snapshot();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].op, Op::Judge);
        assert_eq!(rows[0].input_tokens, 20);
        assert_eq!(rows[0].output_tokens, 10);
        assert_eq!(rows[0].candidate_scale, 6);
        assert!(!rows[0].from_cache);
    }
}
