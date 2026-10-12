//! jev-compatible endpoint backend (R4.T2).
//!
//! Naming rule: this file is the capability surface — the
//! client, config and every public method are `Judge`
//! vocabulary. Only the private [`wire`] module (the
//! byte-level protocol types that mirror the jev wire
//! format 1:1) uses jev names, because those types ARE
//! the wire format.
//!
//! Wire construction, question ids, validation rules and
//! thresholds are ported from the reference implementation
//! (`@jkudish/jev-mcp` `server.js` / `lib.js`, see the
//! per-section notes) so a jev-compatible endpoint answers
//! both identically.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::envelope::{
    self, retry_after_header, CallMeta, EgressGate, MeteringSink, Op, Policy, ProviderError,
    RawCallError,
};
use crate::llm::judge::{
    AspectVerdict, ClassifyClass, ClassifyItem, ClassifyRequest, ClassifyResponse, ClassifyVerdict,
    CompareRelation, CompareRequest, CompareResponse, ExistsVerdict, ExtractField, ExtractRequest,
    ExtractResponse, ExtractResult, ExtractStatus, FindRequest, FindResponse, JudgeClient,
    JudgeError, JudgeResult, RerankHit, RerankRequest, RerankResponse, Verdict,
};

// ---- wire-protocol limits (reference lib.js) --------------------------

/// Find/rerank candidate cap (≤250; callers queue beyond it).
const MAX_CANDIDATES: usize = 250;
/// Rerank aggregate candidate-text budget (characters).
const MAX_RERANK_TOTAL_CHARS: usize = 100_000;
/// Per-candidate text cap (characters).
const MAX_CANDIDATE_CHARS: usize = 2000;
/// Classify items per call.
const MAX_ITEMS: usize = 64;
/// Classify classes per call.
const MAX_CLASSES: usize = 250;
/// Compare aspects per call.
const MAX_COMPARE_ASPECTS: usize = 10;
/// Per-item / per-class description cap.
const MAX_ITEM_CHARS: usize = 2000;
/// Compare passage cap (reference truncates at 20000).
const MAX_PASSAGE_CHARS: usize = 20_000;
/// Extract fields per call.
const MAX_EXTRACT_FIELDS: usize = 32;
/// Extract regex matches kept per field.
const MAX_EXTRACT_CANDIDATES: usize = 20;
/// Extract match length cap.
const MAX_EXTRACT_CANDIDATE_CHARS: usize = 2000;
/// Aggregate extract preview budget.
const MAX_EXTRACT_TOTAL_CHARS: usize = 50_000;
/// Extract document cap.
const MAX_EXTRACT_DOC_CHARS: usize = 50_000;
/// Classify batch budget: items × classes.
const MAX_CLASSIFY_BUDGET: usize = 8_000;
/// Float-safe probability-sum tolerance (reference: 0.01 + 1e-12).
const PROBABILITY_SUM_TOLERANCE: f64 = 0.01 + 1e-12;
/// Minimum top probability for an `auto` decision.
const AUTO_ACCEPT: f64 = 0.85;
/// Minimum winner-to-runner-up gap for an `auto` decision.
const MINIMUM_MARGIN: f64 = 0.5;
/// `exists` noul ≥ this → `Answered` (reference `found`).
const EXISTS_FOUND: f64 = 0.7;
/// `exists` noul < this → `Absent` (reference `absent`).
const EXISTS_ABSENT: f64 = 0.35;

/// The jev wire request: model + shared state + per-judgment
/// questions. Field names mirror the wire format 1:1.
/// The protocol's `Score` question type has no current
/// operation; it stays for wire fidelity.
#[allow(dead_code)] // wire fidelity: the protocol's Score question type has no operation yet
mod wire {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[derive(Serialize, Clone)]
    pub struct JevRequest {
        pub model: String,
        pub state: Value,
        pub questions: BTreeMap<String, JevQuestion>,
    }

    #[derive(Serialize, Clone)]
    #[serde(tag = "type")]
    pub enum JevQuestion {
        Noul {
            instructions: Value,
            criteria: NoulCriteria,
        },
        Choice {
            instructions: Value,
            criteria: BTreeMap<String, Option<String>>,
        },
        Score {
            instructions: Value,
            criteria: Vec<String>,
        },
    }

    #[derive(Serialize, Clone)]
    pub struct NoulCriteria {
        #[serde(rename = "true")]
        pub true_description: Option<String>,
        #[serde(rename = "false")]
        pub false_description: Option<String>,
    }

    #[derive(Deserialize, Clone)]
    pub struct JevResponse {
        pub answers: BTreeMap<String, JevAnswer>,
        #[serde(default)]
        pub usage: Option<JevUsage>,
        #[serde(default)]
        pub model: Option<String>,
    }

    #[derive(Deserialize, Clone)]
    #[serde(untagged)]
    pub enum JevAnswer {
        Choice(JevChoiceAnswer),
        Noul(JevNoulAnswer),
        Score(JevScoreAnswer),
    }

    #[derive(Deserialize, Clone)]
    pub struct JevChoiceAnswer {
        pub choice: String,
        pub probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        pub confidence: Option<f64>,
    }

    #[derive(Deserialize, Clone)]
    pub struct JevNoulAnswer {
        pub noul: f64,
    }

    #[derive(Deserialize, Clone)]
    pub struct JevScoreAnswer {
        pub score: u32,
    }

    #[derive(Deserialize, Clone, Default)]
    pub struct JevUsage {
        #[serde(default, alias = "prompt_tokens")]
        pub input_tokens: u64,
        #[serde(default, alias = "completion_tokens")]
        pub output_tokens: u64,
    }
}

/// Backend configuration. The API key is read from
/// `api_key_env` at construction — the value never enters
/// config files, logs, or any artifact.
#[derive(Debug, Clone)]
pub struct JudgeEndpointConfig {
    /// Full endpoint URL (the jev-compatible judgment API).
    pub endpoint: String,
    /// Environment variable NAME holding the API key.
    pub api_key_env: String,
    /// Fixed model version (cache key + metering row).
    pub model: String,
    pub policy: Policy,
    /// Batch limit for classify/extract (超限分批，不截断).
    pub batch_limit: usize,
    /// In-flight judgment round trips. Further calls
    /// queue (排队而非截断); pending is unbounded —
    /// the contract is never to drop a call.
    pub concurrency: usize,
}

impl JudgeEndpointConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key_env: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key_env: api_key_env.into(),
            model: model.into(),
            policy: Policy::default(),
            batch_limit: 64,
            concurrency: 4,
        }
    }
}

pub struct JudgeEndpointClient {
    config: JudgeEndpointConfig,
    http: reqwest::Client,
    api_key: String,
    gate: EgressGate,
    sink: Arc<dyn MeteringSink>,
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl JudgeEndpointClient {
    /// Reads the API key from `config.api_key_env`.
    pub fn new(
        config: JudgeEndpointConfig,
        http: reqwest::Client,
        gate: EgressGate,
        sink: Arc<dyn MeteringSink>,
    ) -> Result<Self, std::env::VarError> {
        let api_key = std::env::var(&config.api_key_env)?;
        let concurrency = config.concurrency.max(1);
        Ok(Self {
            config,
            http,
            api_key,
            gate,
            sink,
            semaphore: Arc::new(tokio::sync::Semaphore::new(concurrency)),
        })
    }

    /// One judgment round trip through the envelope:
    /// egress gate (opt-in + credential precheck), then
    /// retry/timeout/metering, then the raw response.
    async fn ask(
        &self,
        state: Value,
        questions: BTreeMap<String, wire::JevQuestion>,
        candidate_scale: usize,
    ) -> JudgeResult<wire::JevResponse> {
        // Gate: judge not opted in → PolicyDenied before any
        // network I/O (zero-egress, AC1).
        self.gate.check(Op::Judge).map_err(JudgeError::Backend)?;
        // Credential precheck over the outbound payload (state
        // + questions); precheck results are never logged.
        let payload = json!({ "state": state, "questions": questions });
        self.gate
            .precheck(&payload.to_string())
            .map_err(JudgeError::Backend)?;

        // Concurrency bound: in-flight round trips are
        // capped; excess calls queue here (排队而非截断).
        let _permit = match self.semaphore.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                return Err(JudgeError::Backend(ProviderError::Unavailable {
                    op: Op::Judge,
                }))
            }
        };

        let endpoint = self.config.endpoint.clone();
        let api_key = self.api_key.clone();
        let model = self.config.model.clone();
        let http = self.http.clone();
        let result = envelope::execute(
            Op::Judge,
            &self.config.policy,
            &self.gate,
            self.sink.as_ref(),
            || {
                let body = wire::JevRequest {
                    model: model.clone(),
                    state: state.clone(),
                    questions: questions.clone(),
                };
                let http = http.clone();
                let endpoint = endpoint.clone();
                let api_key = api_key.clone();
                let model = model.clone();
                async move {
                    let response = http
                        .post(&endpoint)
                        .bearer_auth(&api_key)
                        .json(&body)
                        .send()
                        .await
                        .map_err(|_| RawCallError::Transport)?;
                    let status = response.status();
                    let retry_after = retry_after_header(response.headers());
                    let text = response.text().await.unwrap_or_default();
                    if status.is_success() {
                        let response: wire::JevResponse = serde_json::from_str(&text)
                            .map_err(|_| RawCallError::InvalidResponse)?;
                        let usage = response.usage.clone().unwrap_or_default();
                        // usage/model 未报 → 0/0 与配置值（CallMeta 注明）
                        let actual_model = response.model.clone().unwrap_or_else(|| model.clone());
                        let meta = CallMeta {
                            model: actual_model,
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            candidate_scale,
                            from_cache: false,
                        };
                        Ok((response, meta))
                    } else {
                        Err(RawCallError::Http {
                            status: status.as_u16(),
                            retry_after,
                        })
                    }
                }
            },
        )
        .await;
        match result {
            Ok((response, _meta)) => Ok(response),
            // 响应畸形 → InvalidResponse（不重试，修调用）
            Err(ProviderError::InvalidResponse { .. }) => Err(JudgeError::InvalidResponse),
            Err(error) => Err(JudgeError::Backend(error)),
        }
    }
    /// One wire call for a bounded slice of items.
    async fn classify_batch(
        &self,
        items: &[ClassifyItem],
        catalog: &[ClassifyClass],
        purpose: &Option<String>,
    ) -> JudgeResult<Vec<ClassifyVerdict>> {
        if items.len() * catalog.len() > MAX_CLASSIFY_BUDGET {
            return Err(JudgeError::InvalidResponse);
        }
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let mut questions = BTreeMap::new();
        for (i, item) in items.iter().enumerate() {
            let criteria: BTreeMap<String, Option<String>> = catalog
                .iter()
                .map(|class| (class.id.clone(), Some(class.description.clone())))
                .collect();
            questions.insert(
                format!("cls_{i}"),
                wire::JevQuestion::Choice {
                    instructions: truncate(&item.text, MAX_ITEM_CHARS).into(),
                    criteria,
                },
            );
        }
        let state = json!({
            "purpose": purpose.as_deref().map(|p| truncate(p, MAX_ITEM_CHARS)),
        });
        let response = self.ask(state, questions, items.len()).await?;
        let expected: Vec<String> = catalog.iter().map(|class| class.id.clone()).collect();
        let mut verdicts = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let answer =
                validate_choice_answer(response.answers.get(&format!("cls_{i}")), &expected)
                    .ok_or(JudgeError::InvalidResponse)?;
            let probability = answer
                .probabilities
                .get(&answer.choice)
                .copied()
                .unwrap_or(0.0);
            let margin = margin_of(&answer.probabilities);
            verdicts.push(ClassifyVerdict {
                item_id: item.id.clone(),
                class_id: answer.choice.clone(),
                verdict: classification_decision(probability, margin),
                probability,
            });
        }
        Ok(verdicts)
    }

    /// One extract round trip for a bounded slice of
    /// fields. Client-side regex candidate generation
    /// runs per field over the (already truncated)
    /// document; zero-match fields never reach the
    /// model.
    async fn extract_chunk(
        &self,
        fields: &[ExtractField],
        document: &str,
    ) -> JudgeResult<Vec<ExtractResult>> {
        // Client-side regex candidate generation. Zero-match
        // fields never reach the model (NotFound without a
        // call); error fields are reported as Review (the DTO
        // has no invalid_pattern variant).
        struct FieldRun {
            key: String,
            run: ExtractRun,
        }
        let mut runs = Vec::with_capacity(fields.len());
        let mut questions = BTreeMap::new();
        let mut state_fields = Vec::new();
        let mut total_preview_chars = 0usize;
        for (i, field) in fields.iter().enumerate() {
            let key = format!("f{i}");
            let run = run_extract_regex(document, &field.pattern);
            if run.error.is_some() || run.candidates.is_empty() {
                runs.push(FieldRun {
                    key,
                    run: ExtractRun {
                        candidates: Vec::new(),
                        truncated: run.truncated,
                        too_long: run.too_long,
                        error: run.error,
                    },
                });
                continue;
            }
            total_preview_chars += run
                .candidates
                .iter()
                .map(|c| c.chars().count())
                .sum::<usize>();
            let mut criteria = BTreeMap::new();
            for (j, candidate) in run.candidates.iter().enumerate() {
                criteria.insert(
                    format!("c{j}"),
                    Some(format!(
                        "Candidate value: {}",
                        serde_json::to_string(candidate).unwrap_or_default()
                    )),
                );
            }
            criteria.insert(
                "none_of_them".to_string(),
                Some("None of the candidates is the value this field asks for".into()),
            );
            questions.insert(
                key.clone(),
                wire::JevQuestion::Choice {
                    instructions: format!(
                        "Which candidate is the correct value of the field \"{}\" ({}) in the document in the state? Pick the exact substring the document presents as this field's value.",
                        field.id, field.description
                    )
                    .into(),
                    criteria,
                },
            );
            state_fields.push(json!({
                "id": key,
                "description": truncate(&field.description, MAX_ITEM_CHARS),
                "pattern": field.pattern,
            }));
            runs.push(FieldRun {
                key,
                run: ExtractRun {
                    candidates: run.candidates,
                    truncated: run.truncated,
                    too_long: run.too_long,
                    error: None,
                },
            });
        }
        if total_preview_chars > MAX_EXTRACT_TOTAL_CHARS {
            return Err(JudgeError::InvalidResponse);
        }

        // Every field zero-match/error → no call at all.
        if questions.is_empty() {
            return Ok(fields
                .iter()
                .zip(&runs)
                .map(|(field, run)| extract_local_result(field, &run.run))
                .collect());
        }

        let state = json!({
            "purpose": Value::Null,
            "document": document,
            "fields": state_fields,
        });
        let response = self.ask(state, questions, fields.len()).await?;

        let mut results = Vec::with_capacity(fields.len());
        for (field, run) in fields.iter().zip(&runs) {
            if run.run.error.is_some() {
                results.push(ExtractResult {
                    field_id: field.id.clone(),
                    value: None,
                    status: ExtractStatus::Review,
                });
                continue;
            }
            if run.run.candidates.is_empty() {
                results.push(extract_local_result(field, &run.run));
                continue;
            }
            // An incomplete candidate universe (capped or
            // overlong-skipped matches) poisons every
            // outcome, including none_of_them.
            let incomplete = run.run.truncated || run.run.too_long > 0;
            let expected_keys: Vec<String> = (0..run.run.candidates.len())
                .map(|j| format!("c{j}"))
                .chain(["none_of_them".to_string()])
                .collect();
            let answer = validate_choice_answer(response.answers.get(&run.key), &expected_keys)
                .ok_or(JudgeError::InvalidResponse)?;
            let top_probability = answer
                .probabilities
                .get(&answer.choice)
                .copied()
                .unwrap_or(0.0);
            let margin = margin_of(&answer.probabilities);
            if answer.choice == "none_of_them" {
                let status = if incomplete {
                    ExtractStatus::Review
                } else if classification_decision(top_probability, margin) == Verdict::Yes {
                    ExtractStatus::NotFound
                } else {
                    ExtractStatus::Review
                };
                results.push(ExtractResult {
                    field_id: field.id.clone(),
                    value: None,
                    status,
                });
            } else {
                let status = if incomplete {
                    ExtractStatus::Review
                } else {
                    match classification_decision(top_probability, margin) {
                        Verdict::Yes => ExtractStatus::Auto,
                        Verdict::Review => ExtractStatus::Review,
                        // classify never returns No; unreachable here
                        Verdict::No => ExtractStatus::Review,
                    }
                };
                let value = run
                    .run
                    .candidates
                    .get(
                        answer
                            .choice
                            .strip_prefix('c')
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(usize::MAX),
                    )
                    .cloned();
                results.push(ExtractResult {
                    field_id: field.id.clone(),
                    value,
                    status,
                });
            }
        }
        Ok(results)
    }
}

// ---- reference helpers (ported) --------------------------------------

/// Reference `truncate`: cap with an explicit marker.
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    format!(
        "{} […truncated]",
        text.chars().take(max_chars).collect::<String>()
    )
}

/// Reference `sanitizeId`: non-id runs → one `_`,
/// leading/trailing `_` trimmed, ≤64.
fn sanitize_id(id: &str) -> String {
    let mut cleaned = String::with_capacity(id.len());
    let mut pending_underscore = false;
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
            cleaned.push(c);
            pending_underscore = false;
        } else if !pending_underscore {
            cleaned.push('_');
            pending_underscore = true;
        }
    }
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        String::new()
    } else {
        trimmed.chars().take(64).collect()
    }
}

/// Reference `ensureUniqueIds`: sanitize, dedupe with `_n`.
fn ensure_unique_ids(ids: Vec<String>, fallback_prefix: &str) -> Vec<String> {
    let mut used = HashSet::new();
    ids.into_iter()
        .enumerate()
        .map(|(i, raw)| {
            let base = sanitize_id(&raw);
            let base = if base.is_empty() {
                format!("{fallback_prefix}{i}")
            } else {
                base
            };
            let mut id = base.clone();
            let mut n = 1u32;
            while used.contains(&id) {
                id = format!("{base}_{n}");
                n += 1;
            }
            used.insert(id.clone());
            id
        })
        .collect()
}

/// Reference `validateChoiceAnswer`: shape, key-set equality,
/// finite [0,1] probabilities, probability-sum tolerance and
/// the argmax check. `None` → `InvalidResponse`.
fn validate_choice_answer<'a>(
    answer: Option<&'a wire::JevAnswer>,
    expected_keys: &[String],
) -> Option<&'a wire::JevChoiceAnswer> {
    let wire::JevAnswer::Choice(answer) = answer? else {
        return None;
    };
    let expected: HashSet<&str> = expected_keys.iter().map(|k| k.as_str()).collect();
    if !expected.contains(answer.choice.as_str()) {
        return None;
    }
    let keys: HashSet<&str> = answer.probabilities.keys().map(|k| k.as_str()).collect();
    if keys != expected {
        return None;
    }
    let values: Vec<f64> = answer.probabilities.values().copied().collect();
    if !values
        .iter()
        .all(|p| p.is_finite() && (0.0..=1.0).contains(p))
    {
        return None;
    }
    let sum: f64 = values.iter().sum();
    if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return None;
    }
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if answer
        .probabilities
        .get(&answer.choice)
        .copied()
        .unwrap_or(0.0)
        < max - 1e-9
    {
        return None;
    }
    if let Some(confidence) = answer.confidence {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return None;
        }
    }
    Some(answer)
}

/// Reference `validateNoulAnswer`: finite [0,1] or `None`.
fn validate_noul_answer(answer: Option<&wire::JevAnswer>) -> Option<f64> {
    let wire::JevAnswer::Noul(answer) = answer? else {
        return None;
    };
    let value = answer.noul;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Some(value)
    } else {
        None
    }
}

/// Reference `classificationDecision`.
fn classification_decision(top_probability: f64, margin: f64) -> Verdict {
    if top_probability >= AUTO_ACCEPT && margin >= MINIMUM_MARGIN {
        Verdict::Yes
    } else {
        Verdict::Review
    }
}

/// Reference `marginOf`: winner-to-runner-up gap (lone → 0).
fn margin_of(probabilities: &BTreeMap<String, f64>) -> f64 {
    let mut ranked: Vec<f64> = probabilities.values().copied().collect();
    ranked.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    if ranked.len() >= 2 {
        ranked[0] - ranked[1]
    } else {
        0.0
    }
}

/// Reference `existsVerdict` (found = 0.7, absent = 0.35).
fn exists_verdict(exists: f64) -> ExistsVerdict {
    if exists >= EXISTS_FOUND {
        ExistsVerdict::Answered
    } else if exists < EXISTS_ABSENT {
        ExistsVerdict::Absent
    } else {
        ExistsVerdict::Partial
    }
}

/// Answers must be exactly the expected question ids — a
/// missing or extra id is a malformed response.
fn check_answer_ids(
    answers: &BTreeMap<String, wire::JevAnswer>,
    expected: &[String],
) -> JudgeResult<()> {
    if answers.len() != expected.len() || !expected.iter().all(|qid| answers.contains_key(qid)) {
        return Err(JudgeError::InvalidResponse);
    }
    Ok(())
}

fn compare_relations() -> BTreeMap<String, Option<String>> {
    [
        (
            "same_fact",
            "Both passages state the same underlying fact or claim",
        ),
        (
            "contradicts",
            "The passages state opposing facts about the same subject",
        ),
        (
            "different_facts",
            "The passages discuss different subjects or make non-overlapping claims",
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Some(v.to_string())))
    .collect()
}

fn aspect_relations() -> BTreeMap<String, Option<String>> {
    [
        (
            "same_fact",
            "Both passages make comparable assertions about this aspect and they agree",
        ),
        (
            "contradicts",
            "Both passages address this aspect and their assertions conflict",
        ),
        (
            "different_facts",
            "The passages do not both make a comparable assertion about this aspect: at least one does not address it, or their mentions do not overlap",
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Some(v.to_string())))
    .collect()
}

fn compare_relation(answer: &wire::JevChoiceAnswer) -> JudgeResult<CompareRelation> {
    match answer.choice.as_str() {
        "same_fact" => Ok(CompareRelation::SameFact),
        "contradicts" => Ok(CompareRelation::Contradicts),
        "different_facts" => Ok(CompareRelation::DifferentFacts),
        _ => Err(JudgeError::InvalidResponse),
    }
}

// ---- extract: client-side regex candidate generation ------------------

struct ExtractRun {
    candidates: Vec<String>,
    truncated: bool,
    too_long: u32,
    error: Option<String>,
}

/// Port of the reference regex worker: zero-length matches
/// dropped, overlong matches skipped (counted), candidates
/// capped (truncated flag), invalid pattern → error. The
/// Rust `regex` crate is linear-time, so the reference's
/// sandboxed worker and hard timeout need no porting.
fn run_extract_regex(document: &str, pattern: &str) -> ExtractRun {
    let re = match regex::Regex::new(pattern) {
        Ok(re) => re,
        Err(error) => {
            return ExtractRun {
                candidates: Vec::new(),
                truncated: false,
                too_long: 0,
                error: Some(error.to_string()),
            }
        }
    };
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let mut truncated = false;
    let mut too_long = 0u32;
    for matched in re.find_iter(document) {
        let value = matched.as_str();
        if value.is_empty() || seen.contains(value) {
            continue;
        }
        seen.insert(value.to_string());
        if value.chars().count() > MAX_EXTRACT_CANDIDATE_CHARS {
            too_long += 1;
            continue;
        }
        if candidates.len() >= MAX_EXTRACT_CANDIDATES {
            truncated = true;
            break;
        }
        candidates.push(value.to_string());
    }
    ExtractRun {
        candidates,
        truncated,
        too_long,
        error: None,
    }
}

// ---- JudgeClient -----------------------------------------------------

#[async_trait]
impl JudgeClient for JudgeEndpointClient {
    fn model_id(&self) -> String {
        self.config.model.clone()
    }

    async fn classify(&self, req: &ClassifyRequest) -> JudgeResult<ClassifyResponse> {
        if req.catalog.len() > MAX_CLASSES {
            return Err(JudgeError::InvalidResponse);
        }
        let mut verdicts = Vec::with_capacity(req.items.len());
        for chunk in req
            .items
            .chunks(self.config.batch_limit.clamp(1, MAX_ITEMS))
        {
            verdicts.extend(
                self.classify_batch(chunk, &req.catalog, &req.purpose)
                    .await?,
            );
        }
        let auto = verdicts.iter().all(|v| v.verdict == Verdict::Yes);
        Ok(ClassifyResponse { verdicts, auto })
    }

    async fn compare(&self, req: &CompareRequest) -> JudgeResult<CompareResponse> {
        if req.aspects.len() > MAX_COMPARE_ASPECTS {
            return Err(JudgeError::InvalidResponse);
        }
        let passage_a = truncate(&req.passage_a, MAX_PASSAGE_CHARS);
        let passage_b = truncate(&req.passage_b, MAX_PASSAGE_CHARS);

        let mut questions = BTreeMap::new();
        questions.insert(
            "overall".to_string(),
            wire::JevQuestion::Choice {
                instructions: "Do the two passages state the same underlying fact, contradict each other, or discuss different facts?".into(),
                criteria: compare_relations(),
            },
        );
        for (i, aspect) in req.aspects.iter().enumerate() {
            questions.insert(
                format!("aspect_{i}"),
                wire::JevQuestion::Choice {
                    instructions: format!(
                        "Judging only the aspect \"{aspect}\" of the two passages in the state, which relation holds?"
                    )
                    .into(),
                    criteria: aspect_relations(),
                },
            );
        }
        let state = json!({
            "purpose": Value::Null,
            "passage_a": passage_a,
            "passage_b": passage_b,
            "aspects": req.aspects,
        });
        let response = self.ask(state, questions, 2 + req.aspects.len()).await?;

        let mut expected = vec!["overall".to_string()];
        expected.extend((0..req.aspects.len()).map(|i| format!("aspect_{i}")));
        check_answer_ids(&response.answers, &expected)?;

        let relation_keys: Vec<String> = compare_relations().keys().cloned().collect();
        let overall = validate_choice_answer(response.answers.get("overall"), &relation_keys)
            .ok_or(JudgeError::InvalidResponse)?;
        let relation = compare_relation(overall)?;
        let confidence = overall.confidence.unwrap_or_else(|| {
            overall
                .probabilities
                .get(&overall.choice)
                .copied()
                .unwrap_or(0.0)
        });

        let mut aspects = Vec::with_capacity(req.aspects.len());
        for (i, aspect) in req.aspects.iter().enumerate() {
            let answer = validate_choice_answer(
                response.answers.get(&format!("aspect_{i}")),
                &relation_keys,
            )
            .ok_or(JudgeError::InvalidResponse)?;
            aspects.push(AspectVerdict {
                aspect: aspect.clone(),
                relation: compare_relation(answer)?,
                confidence: answer.confidence.unwrap_or_else(|| {
                    answer
                        .probabilities
                        .get(&answer.choice)
                        .copied()
                        .unwrap_or(0.0)
                }),
            });
        }
        Ok(CompareResponse {
            relation,
            confidence,
            aspects,
        })
    }

    async fn extract(&self, req: &ExtractRequest) -> JudgeResult<ExtractResponse> {
        let document = truncate(&req.document, MAX_EXTRACT_DOC_CHARS);
        // Batch splitting: fields beyond `batch_limit`
        // queue into sequential wire calls （超限分批，
        // 不截断不拒绝）; each call stays within the
        // wire's own field cap.
        let chunk = self.config.batch_limit.clamp(1, MAX_EXTRACT_FIELDS);
        let mut results = Vec::with_capacity(req.fields.len());
        for fields in req.fields.chunks(chunk) {
            results.extend(self.extract_chunk(fields, &document).await?);
        }
        Ok(ExtractResponse { fields: results })
    }

    async fn rerank(&self, req: &RerankRequest) -> JudgeResult<RerankResponse> {
        if req.candidates.len() > MAX_CANDIDATES {
            return Err(JudgeError::InvalidResponse);
        }
        let total_chars: usize = req.candidates.iter().map(|c| c.text.chars().count()).sum();
        if total_chars > MAX_RERANK_TOTAL_CHARS {
            return Err(JudgeError::InvalidResponse);
        }
        // The query lives once in shared state; each question
        // carries only its own candidate text.
        let mut questions = BTreeMap::new();
        for (i, candidate) in req.candidates.iter().enumerate() {
            let key = format!("rel_{i}");
            let instructions = format!(
                "Is candidate {key} relevant to the query in the state? Candidate {key}: {}",
                truncate(&candidate.text, MAX_CANDIDATE_CHARS)
            );
            questions.insert(
                key,
                wire::JevQuestion::Noul {
                    instructions: instructions.into(),
                    criteria: wire::NoulCriteria {
                        true_description: Some(
                            "The candidate addresses the subject the query asks about, or provides what it seeks".into(),
                        ),
                        false_description: Some(
                            "The candidate is about a different subject, or only shares vocabulary with the query".into(),
                        ),
                    },
                },
            );
        }
        let state = json!({ "query": req.query });
        let response = self.ask(state, questions, req.candidates.len()).await?;

        let expected: Vec<String> = (0..req.candidates.len())
            .map(|i| format!("rel_{i}"))
            .collect();
        check_answer_ids(&response.answers, &expected)?;

        // One invalid noul makes the whole ordering
        // untrustworthy; a missing answer is never a
        // confident zero.
        let mut hits = Vec::with_capacity(req.candidates.len());
        for (i, candidate) in req.candidates.iter().enumerate() {
            let relevance = validate_noul_answer(response.answers.get(&format!("rel_{i}")))
                .ok_or(JudgeError::InvalidResponse)?;
            hits.push(RerankHit {
                id: candidate.id.clone(),
                relevance,
            });
        }
        hits.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(RerankResponse { ranked: hits })
    }

    async fn find(&self, req: &FindRequest) -> JudgeResult<FindResponse> {
        if req.candidates.is_empty() {
            return Ok(FindResponse {
                best: None,
                exists_verdict: ExistsVerdict::Absent,
            });
        }
        if req.candidates.len() > MAX_CANDIDATES {
            return Err(JudgeError::InvalidResponse);
        }
        let candidate_keys =
            ensure_unique_ids(req.candidates.iter().map(|c| c.id.clone()).collect(), "c");
        let mut criteria = BTreeMap::new();
        for key in &candidate_keys {
            criteria.insert(key.clone(), None);
        }
        let questions = BTreeMap::from([
            (
                "best".to_string(),
                wire::JevQuestion::Choice {
                    instructions: format!(
                        "Which candidate contains the best answer to: \"{}\"?",
                        req.query
                    )
                    .into(),
                    criteria: criteria.clone(),
                },
            ),
            (
                "exists".to_string(),
                wire::JevQuestion::Noul {
                    instructions: format!(
                        "Does any candidate address or answer: \"{}\"?",
                        req.query
                    )
                    .into(),
                    criteria: wire::NoulCriteria {
                        true_description: Some(
                            "At least one candidate states or directly implies the answer".into(),
                        ),
                        false_description: Some("No candidate addresses this".into()),
                    },
                },
            ),
        ]);
        let candidates_json: Vec<Value> = req
            .candidates
            .iter()
            .zip(&candidate_keys)
            .map(|(candidate, key)| {
                json!({
                    "id": key,
                    "text": truncate(&candidate.text, MAX_CANDIDATE_CHARS),
                })
            })
            .collect();
        let state = json!({ "query": req.query, "candidates": candidates_json });
        let response = self.ask(state, questions, req.candidates.len()).await?;
        check_answer_ids(
            &response.answers,
            &["best".to_string(), "exists".to_string()],
        )?;

        // exists 与 best 任一缺失 → InvalidResponse（缺失不得
        // 读作「无匹配」或「无排序」）.
        let exists = validate_noul_answer(response.answers.get("exists"))
            .ok_or(JudgeError::InvalidResponse)?;
        let best = validate_choice_answer(response.answers.get("best"), &candidate_keys)
            .ok_or(JudgeError::InvalidResponse)?;
        let key_to_id: BTreeMap<&str, &str> = candidate_keys
            .iter()
            .zip(req.candidates.iter())
            .map(|(key, candidate)| (key.as_str(), candidate.id.as_str()))
            .collect();
        let best_id = key_to_id
            .get(best.choice.as_str())
            .map(|id| (*id).to_string())
            .ok_or(JudgeError::InvalidResponse)?;
        let relevance = best.probabilities.get(&best.choice).copied().unwrap_or(0.0);
        Ok(FindResponse {
            best: Some(RerankHit {
                id: best_id,
                relevance,
            }),
            exists_verdict: exists_verdict(exists),
        })
    }
}

/// Local (no-call) extract result for zero-match / error /
/// overlong-only fields.
fn extract_local_result(
    field: &crate::llm::judge::ExtractField,
    run: &ExtractRun,
) -> ExtractResult {
    if run.error.is_some() {
        // Invalid pattern: conservative Review (the DTO has no
        // invalid_pattern variant; the constant catalogs are
        // validated at startup so this is unreachable in
        // practice).
        ExtractResult {
            field_id: field.id.clone(),
            value: None,
            status: ExtractStatus::Review,
        }
    } else if run.too_long > 0 {
        // Matches exist but were all overlong: the universe is
        // incomplete, so the field is Review, not NotFound.
        ExtractResult {
            field_id: field.id.clone(),
            value: None,
            status: ExtractStatus::Review,
        }
    } else {
        ExtractResult {
            field_id: field.id.clone(),
            value: None,
            status: ExtractStatus::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::judge::ExtractField;

    fn field(id: &str, pattern: &str) -> ExtractField {
        ExtractField {
            id: id.into(),
            pattern: pattern.into(),
            description: "test field".into(),
        }
    }

    #[test]
    fn truncate_caps_with_marker() {
        let short = truncate("hello", 10);
        assert_eq!(short, "hello");
        let long = truncate("hello world", 5);
        assert_eq!(long, "hello […truncated]");
    }

    #[test]
    fn sanitize_id_matches_reference() {
        assert_eq!(sanitize_id("clean-id.1"), "clean-id.1");
        assert_eq!(sanitize_id("bad id!"), "bad_id");
        assert_eq!(sanitize_id("a!!b"), "a_b");
        assert_eq!(sanitize_id(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn ensure_unique_ids_dedupes() {
        let ids = ensure_unique_ids(vec!["a".into(), "a".into(), "".into()], "i");
        assert_eq!(ids, vec!["a", "a_1", "i2"]);
    }

    #[test]
    fn margin_of_is_winner_minus_runner_up() {
        let mut probabilities = BTreeMap::new();
        probabilities.insert("a".to_string(), 0.7);
        probabilities.insert("b".to_string(), 0.2);
        probabilities.insert("c".to_string(), 0.1);
        assert!((margin_of(&probabilities) - 0.5).abs() < 1e-12);
        let mut lone = BTreeMap::new();
        lone.insert("a".to_string(), 1.0);
        assert_eq!(margin_of(&lone), 0.0);
    }

    #[test]
    fn classification_decision_gates_on_probability_and_margin() {
        assert_eq!(classification_decision(0.9, 0.6), Verdict::Yes);
        assert_eq!(classification_decision(0.9, 0.4), Verdict::Review);
        assert_eq!(classification_decision(0.8, 0.6), Verdict::Review);
    }

    #[test]
    fn exists_verdict_thresholds() {
        assert_eq!(exists_verdict(0.9), ExistsVerdict::Answered);
        assert_eq!(exists_verdict(0.5), ExistsVerdict::Partial);
        assert_eq!(exists_verdict(0.2), ExistsVerdict::Absent);
    }

    #[test]
    fn extract_regex_ports_the_reference_worker() {
        let document = "see ERR-1234 and ERR-1234 and EAI_AGAIN";
        let run = run_extract_regex(document, r"\b[A-Z][A-Z0-9]*(?:[-_][A-Z0-9]+)+\b");
        assert_eq!(run.candidates, vec!["ERR-1234", "EAI_AGAIN"]);
        assert!(!run.truncated);
        assert_eq!(run.too_long, 0);
        assert!(run.error.is_none());

        // zero-length matches dropped, dedup, overlong skipped
        let run = run_extract_regex("a ab ab abc", r"ab?");
        assert_eq!(run.candidates, vec!["a", "ab"]);
        let run = run_extract_regex("x", r"(?P<empty>)");
        assert!(run.candidates.is_empty());
        let long = "y".repeat(MAX_EXTRACT_CANDIDATE_CHARS + 1);
        let run = run_extract_regex(&long, "y+");
        assert!(run.candidates.is_empty());
        assert_eq!(run.too_long, 1);
        let run = run_extract_regex("text", r"(");
        assert!(run.error.is_some());
    }

    #[test]
    fn extract_regex_caps_candidates() {
        let document = (0..30).map(|i| format!("tok{i} ")).collect::<String>();
        let run = run_extract_regex(&document, r"tok\d+");
        assert_eq!(run.candidates.len(), MAX_EXTRACT_CANDIDATES);
        assert!(run.truncated);
    }

    #[test]
    fn validate_choice_answer_enforces_the_reference_rules() {
        let keys = vec!["a".to_string(), "b".to_string()];
        let mut probabilities = BTreeMap::new();
        probabilities.insert("a".to_string(), 0.8);
        probabilities.insert("b".to_string(), 0.2);
        let answer = wire::JevAnswer::Choice(wire::JevChoiceAnswer {
            choice: "a".into(),
            probabilities: probabilities.clone(),
            confidence: Some(0.9),
        });
        assert!(validate_choice_answer(Some(&answer), &keys).is_some());

        // sum ≠ 1
        let mut bad = probabilities.clone();
        bad.insert("b".to_string(), 0.3);
        let answer = wire::JevAnswer::Choice(wire::JevChoiceAnswer {
            choice: "a".into(),
            probabilities: bad,
            confidence: None,
        });
        assert!(validate_choice_answer(Some(&answer), &keys).is_none());

        // non-argmax choice
        let answer = wire::JevAnswer::Choice(wire::JevChoiceAnswer {
            choice: "b".into(),
            probabilities,
            confidence: None,
        });
        assert!(validate_choice_answer(Some(&answer), &keys).is_none());

        // unknown key
        let answer = wire::JevAnswer::Choice(wire::JevChoiceAnswer {
            choice: "z".into(),
            probabilities: BTreeMap::new(),
            confidence: None,
        });
        assert!(validate_choice_answer(Some(&answer), &keys).is_none());
    }

    #[test]
    fn validate_noul_answer_enforces_bounds() {
        let good = wire::JevAnswer::Noul(wire::JevNoulAnswer { noul: 0.5 });
        assert_eq!(validate_noul_answer(Some(&good)), Some(0.5));
        let bad = wire::JevAnswer::Noul(wire::JevNoulAnswer { noul: 1.5 });
        assert_eq!(validate_noul_answer(Some(&bad)), None);
        let wrong_shape = wire::JevAnswer::Score(wire::JevScoreAnswer { score: 3 });
        assert_eq!(validate_noul_answer(Some(&wrong_shape)), None);
    }

    #[test]
    fn check_answer_ids_requires_the_exact_set() {
        let mut answers = BTreeMap::new();
        answers.insert(
            "q0".to_string(),
            wire::JevAnswer::Noul(wire::JevNoulAnswer { noul: 0.5 }),
        );
        assert!(check_answer_ids(&answers, &["q0".to_string()]).is_ok());
        assert!(check_answer_ids(&answers, &["q0".to_string(), "q1".to_string()]).is_err());
        answers.insert(
            "q1".to_string(),
            wire::JevAnswer::Noul(wire::JevNoulAnswer { noul: 0.5 }),
        );
        assert!(check_answer_ids(&answers, &["q0".to_string()]).is_err());
    }

    #[test]
    fn extract_local_result_maps_the_reference_statuses() {
        let f = field("error_code", ".*");
        let not_found = ExtractRun {
            candidates: vec![],
            truncated: false,
            too_long: 0,
            error: None,
        };
        assert_eq!(
            extract_local_result(&f, &not_found).status,
            ExtractStatus::NotFound
        );
        let overlong = ExtractRun {
            candidates: vec![],
            truncated: false,
            too_long: 2,
            error: None,
        };
        assert_eq!(
            extract_local_result(&f, &overlong).status,
            ExtractStatus::Review
        );
        let errored = ExtractRun {
            candidates: vec![],
            truncated: false,
            too_long: 0,
            error: Some("x".into()),
        };
        assert_eq!(
            extract_local_result(&f, &errored).status,
            ExtractStatus::Review
        );
    }
}
