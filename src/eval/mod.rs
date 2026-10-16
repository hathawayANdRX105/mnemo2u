//! Evaluation harness — the manifest, the runner, the three-axis report
//! (stage doc 06 T1/T4/T5).
//!
//! The point of this module is a *replayable* number: a case is a corpus
//! plus queries plus frozen success conditions, and running it produces a
//! `CaseResult` that can be compared against the frozen thresholds. The
//! ordering the stage doc mandates — freeze the criteria, then score — is
//! enforced by construction: the snapshot id is a hash of the effective
//! corpus and queries, so editing a corpus changes the snapshot and the
//! frozen expectation has to be updated by hand, in a reviewable diff.
//!
//! ## Scope of this slice
//!
//! Offline and deterministic, because it has to run in CI with no network
//! and no model download:
//!
//! - the write path runs the real [`FactWriteGateway`](crate::index::gateway)
//!   over real turso stores, in the zero-LLM capture mode;
//! - the read path runs the real [`MemoryRecall`](crate::query::facts)
//!   with its lexical, graph and RRF stages.
//!
//! The **vector arm is not part of the gate**: it needs a real embedding
//! model (a network download), and `MockEmbedder`'s token-hash vectors carry
//! no semantics to gate on. Vector-arm behaviour is gated separately by
//! `tests/vector_arm_scope.rs` and the latency baseline in
//! `tests/memory_recall.rs` (AC4).
//!
//! MTRAG (T2) and LongMemEval (T3) are deliberately out of this slice: the
//! stage doc marks their licences "verify before use" and the MTRAG corpus
//! is a 206 MB sparse checkout. They land as a follow-up once the licence
//! question is settled, and the same runner drives them.

pub mod cases;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::core::text::Tokenizer;
use crate::core::traits::Embedder;
use crate::core::types::{FactId, ScopeKey};
use crate::index::gateway::{FactWriteGateway, GatewayOptions, GatewayParts};
use crate::index::ingest::{SessionTurn, Turn};
use crate::llm::mock::MockEmbedder;
use crate::query::facts::{MemoryRecall, RecallOptions, RecallOutcome};
use crate::store::memory::MemoryKv;
use crate::store::turso_edges::TursoFactEdges;
use crate::store::turso_facts::TursoFactStore;
use crate::store::turso_lexical::TursoLexical;

/// One evaluation case: a corpus, its queries, and the frozen conditions
/// the result is judged against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    /// Stable id; two runs of the same id must produce the same snapshot.
    pub case_id: String,
    /// Which case set this belongs to (`focus-switch` | `identifier`).
    pub set: String,
    /// Hash over the effective corpus, queries, scale and frozen
    /// thresholds. Editing any of them changes this, which is how a silent
    /// criteria edit is caught.
    pub snapshot_id: String,
    /// The scope the corpus is written and recalled under.
    pub scope: ScopeKey,
    /// The conversation the content turns arrive in.
    pub conversation_id: String,
    /// The turns the gateway ingests.
    pub turns: Vec<Turn>,
    /// Unrelated sessions inserted as distractors, at `scale`.
    pub distractor_sessions: usize,
    /// The recall queries.
    pub queries: Vec<CaseQuery>,
    /// Frozen success conditions.
    pub conditions: Conditions,
    /// How many recall runs per query (the latency sample size).
    pub repetitions: usize,
}

/// One recall query and what a correct answer looks like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseQuery {
    /// The query text.
    pub text: String,
    /// Markers that must appear in the top-k candidate texts.
    pub expected: Vec<String>,
    /// Markers that must **not** appear in the top-k — wrong evidence is a
    /// quality failure even when the right evidence is also there.
    pub forbidden: Vec<String>,
}

/// The frozen thresholds (stage doc 06: freeze before scoring).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Conditions {
    /// Candidates per query.
    pub top_k: usize,
    /// Fraction of `expected` markers that must be present.
    pub min_recall: f64,
    /// Whether any `forbidden` marker appearing is an immediate failure.
    pub forbid_wrong_evidence: bool,
    /// Whether the **top-ranked** candidate must carry an expected marker.
    /// Distinct from `min_recall`: recall is satisfied by any top-k slot, so
    /// without this a correct fact demoted from rank 1 by a fusion or
    /// per-arm-limit change still passes.
    pub expect_top_hit: bool,
}

impl Default for Conditions {
    fn default() -> Self {
        Self {
            top_k: 5,
            min_recall: 1.0,
            forbid_wrong_evidence: true,
            expect_top_hit: false,
        }
    }
}

/// One query's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub text: String,
    /// Fraction of the expected markers found in the top-k.
    pub recall: f64,
    /// Expected markers that were missing.
    pub missing: Vec<String>,
    /// Forbidden markers that were present.
    pub wrong_evidence: Vec<String>,
    /// The fact ids the recall returned, in rank order.
    pub returned_ids: Vec<FactId>,
    /// Whether the top-ranked candidate carried an expected marker.
    pub top_hit: bool,
}

/// Quality axis (stage doc 06 §三轴报告 1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quality {
    /// Mean recall across the case's queries.
    pub recall_at_k: f64,
    /// Fraction of queries that missed at least one expected marker.
    pub miss_rate: f64,
    /// Fraction of queries that surfaced forbidden evidence.
    pub wrong_evidence_rate: f64,
}

/// Cost axis (stage doc 06 §三轴报告 2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// Embedder invocations (the local model is the only model this slice
    /// calls).
    pub embed_calls: usize,
    /// Texts embedded across those calls.
    pub embedded_texts: usize,
    /// Judge invocations — zero in the zero-LLM capture mode; the judge path
    /// is costed by `MeteringSink` in the judge tests.
    pub judge_calls: usize,
    /// Tokens are `None` when the service reports none (offline mode has no
    /// service).
    pub tokens: Option<u64>,
}

/// Latency axis (stage doc 06 §三轴报告 3), in microseconds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    pub samples: usize,
    pub p50_us: u128,
    pub p95_us: u128,
}

/// Why a case failed, with the evidence attached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Failure {
    /// A query missed expected markers.
    MissedRecall { query: String, missing: Vec<String> },
    /// A query surfaced forbidden markers.
    WrongEvidence { query: String, found: Vec<String> },
    /// Recall returned nothing at all.
    NoCandidates { query: String },
    /// The top-ranked candidate carried no expected marker.
    NotTopRanked { query: String, top: String },
}

/// The full result of running one case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub case_id: String,
    pub snapshot_id: String,
    pub quality: Quality,
    pub cost: Cost,
    pub latency: Latency,
    pub queries: Vec<QueryResult>,
    pub failures: Vec<Failure>,
    /// The case's frozen conditions, echoed so a report is self-describing.
    pub conditions: Conditions,
}

impl CaseResult {
    /// Whether every frozen condition held.
    pub fn passed(&self) -> bool {
        self.failures.is_empty() && self.quality.recall_at_k >= self.conditions.min_recall
    }
}

/// The manifest: the case, the run that executed it, and the result.
/// Serialized as JSON so a run is an artifact that can be diffed and
/// replayed (stage doc 06 §Manifest 契约).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub case: Case,
    pub run: Run,
    pub result: CaseResult,
}

/// Provenance of one run (stage doc 06 `run`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// The commit the binary was built from, when the build recorded one.
    pub code_revision: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub error: Option<String>,
}

/// The commit the binary was built from, recorded at compile time so a
/// manifest never needs a git call. `GITHUB_SHA` is what CI sets;
/// `MNEMO_REVISION` covers other builds.
pub fn code_revision() -> Option<String> {
    option_env!("GITHUB_SHA")
        .or_else(|| option_env!("MNEMO_REVISION"))
        .map(str::to_string)
}

/// A wall-clock stamp, UTC, `YYYY-MM-DDTHH:MM:SSZ`. `time` is a dependency
/// of nothing here on purpose — the manifest only needs a coarse stamp for
/// ordering evidence, so the epoch seconds are formatted directly.
fn stamp(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let seconds_of_day = epoch_seconds.rem_euclid(86_400);
    let (hour, minute, second) = (
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
    );
    // Civil-from-days (Howard Hinnant), the same arithmetic the temporal arm
    // uses, so no date library enters the dependency set for a stamp.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year + era * 400;
    let day_of_year = day_of_era - (365 * year + year / 4 - year / 100 + year / 400);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// The snapshot id: a hash over everything that defines the effective run —
/// corpus, queries, scale *and* the frozen thresholds — so neither a silent
/// corpus edit nor a loosened criterion can pass a frozen expectation.
pub fn snapshot_id(
    case_id: &str,
    turns: &[Turn],
    queries: &[CaseQuery],
    scale: usize,
    conditions: &Conditions,
) -> String {
    let payload = serde_json::json!({
        "case_id": case_id,
        "turns": turns,
        "queries": queries,
        "distractor_sessions": scale,
        "conditions": conditions,
    });
    crate::core::text::compute_mdhash_id(&payload.to_string(), "snap-")
}

/// The distractor corpus for one scale: `scale` unrelated sessions, four
/// turns each, vocabulary that does not appear in any case's content turns.
/// Deterministic in `scale`, so the same scale always builds the same
/// corpus (and therefore the same snapshot).
fn distractor_turns(scale: usize) -> Vec<SessionTurn> {
    let glyphs = [
        "zephyr",
        "quill",
        "monograph",
        "tessellate",
        "bracken",
        "vellum",
        "cobalt",
        "fjord",
    ];
    let mut turns = Vec::with_capacity(scale * 4);
    for session in 0..scale {
        for index in 0..4 {
            let glyph = glyphs[(session + index) % glyphs.len()];
            let other = glyphs[(session * 3 + index * 5 + 1) % glyphs.len()];
            turns.push(SessionTurn {
                conversation_id: format!("distractor-{session}"),
                turn: Turn {
                    // Globally unique, not just unique within a session: the
                    // gateway derives a fact id from the *batch's*
                    // conversation id plus this turn id, so a per-session id
                    // would collapse every session's turns onto the same four
                    // facts and the scale axis would stop growing.
                    turn_id: format!("d{session}-{index}"),
                    role: if index % 2 == 0 { "user" } else { "assistant" }.to_string(),
                    content: format!(
                        "the {glyph} ledger records the {other} reconciliation for quarter {}",
                        session + 1
                    ),
                    timestamp: 1_000_000 + (session * 10 + index) as i64,
                },
            });
        }
    }
    turns
}

/// The stores one evaluation case runs against. Turso for the truth layer
/// and the lexical index, turso adjacency for the edges, an in-memory KV for
/// the watermark — the same durable backends production uses, in a directory
/// the caller owns.
pub struct CaseStores {
    pub facts: Arc<TursoFactStore>,
    pub lexical: Arc<TursoLexical>,
    pub edges: Arc<TursoFactEdges>,
}

/// Open the stores for one case under `workspace`.
pub async fn open_stores(workspace: &Path) -> Result<CaseStores, String> {
    std::fs::create_dir_all(workspace).map_err(|error| error.to_string())?;
    let facts = TursoFactStore::open(&workspace.join("facts.db").to_string_lossy())
        .await
        .map_err(|error| error.to_string())?;
    let lexical = TursoLexical::new(&workspace.join("lexical.db").to_string_lossy())
        .await
        .map_err(|error| error.to_string())?;
    let edges = TursoFactEdges::new(&workspace.join("edges.db").to_string_lossy())
        .await
        .map_err(|error| error.to_string())?;
    Ok(CaseStores {
        facts: Arc::new(facts),
        lexical: Arc::new(lexical),
        edges: Arc::new(edges),
    })
}

/// Run one case end to end: ingest through the gateway, then recall each
/// query, measuring quality, cost and latency.
///
/// `workspace` is where the stores live; pass a fresh directory per run so
/// the case is self-contained (and re-runnable).
pub async fn run_case(case: &Case, workspace: &Path) -> Result<CaseResult, String> {
    let stores = open_stores(workspace).await?;
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new());

    let gateway = FactWriteGateway::new(
        GatewayOptions {
            scope: case.scope.clone(),
            // The value gate is opened for evaluation: the cases are short by
            // construction (a turn is a turn, not a document), and the gate's
            // behaviour is gated separately by the write-gateway tests. What
            // is under test here is recall, not screening.
            cost: crate::index::cost::CostOptions {
                min_chunk_tokens: 1,
                duplicate_similarity: 1.01,
                ..Default::default()
            },
            ..GatewayOptions::default()
        },
        GatewayParts {
            facts: stores.facts.clone(),
            vector: None,
            lexical: Some(stores.lexical.clone()),
            edges: Some(stores.edges.clone()),
            embedder: embedder.clone(),
            judge: None,
            tombstones: Arc::new(MemoryKv::new()),
            watermarks: Arc::new(MemoryKv::new()),
            tokenizer: Tokenizer::for_gpt_4o().map_err(|error| error.to_string())?,
        },
    );

    // Ingest the content turns, then the distractors. Both go through the
    // same gateway, so the write path under test is the production one.
    let content: Vec<SessionTurn> = case
        .turns
        .iter()
        .enumerate()
        .map(|(index, turn)| SessionTurn {
            conversation_id: case.conversation_id.clone(),
            turn: Turn {
                turn_id: format!("t{index}"),
                ..turn.clone()
            },
        })
        .collect();
    // Only the query embeds below are real work: the gateway's embed is
    // gated on a wired vector store and this harness runs the arm off, so
    // counting a call per ingest batch would overstate the cost axis.
    let mut embed_calls = 0usize;
    let mut embedded_texts = 0usize;
    for (conversation, turns) in [
        (case.conversation_id.as_str(), content),
        ("distractors", distractor_turns(case.distractor_sessions)),
    ] {
        gateway
            .commit_turns(conversation, &turns)
            .await
            .map_err(|error| error.to_string())?;
    }

    let recall = MemoryRecall {
        facts: stores.facts.clone(),
        vector: None,
        lexical: Some(stores.lexical.clone()),
        edges: Some(stores.edges.clone()),
        judge: None,
    };
    let options = RecallOptions {
        top_k: case.conditions.top_k,
        rrf_k: 60,
        per_arm_limit: 100,
        staleness: None,
    };

    let mut results = Vec::with_capacity(case.queries.len());
    let mut samples: Vec<u128> = Vec::new();
    let mut failures = Vec::new();
    for query in &case.queries {
        // The query vector is derived from the same local embedder the
        // gateway used, so the vector arm (when enabled) sees a consistent
        // space; with the arm off it is simply unused.
        let query_vec = embedder
            .embed(std::slice::from_ref(&query.text))
            .await
            .map_err(|error| error.to_string())?;
        embed_calls += 1;
        embedded_texts += 1;

        let mut last: Option<crate::core::traits::Result<RecallOutcome>> = None;
        for _ in 0..case.repetitions.max(1) {
            let started = Instant::now();
            let outcome = recall
                .recall(&case.scope, &query.text, Some(&query_vec[0]), &options)
                .await;
            samples.push(started.elapsed().as_micros());
            last = Some(outcome);
        }
        let outcome = last
            .expect("at least one repetition")
            .map_err(|e| e.to_string())?;

        let texts: Vec<String> = outcome
            .candidates
            .iter()
            .map(|candidate| candidate.summary.clone())
            .collect();
        let returned_ids: Vec<FactId> = outcome
            .candidates
            .iter()
            .map(|candidate| candidate.fact_id.clone())
            .collect();

        let missing: Vec<String> = query
            .expected
            .iter()
            .filter(|marker| !texts.iter().any(|text| text.contains(marker.as_str())))
            .cloned()
            .collect();
        let wrong: Vec<String> = query
            .forbidden
            .iter()
            .filter(|marker| texts.iter().any(|text| text.contains(marker.as_str())))
            .cloned()
            .collect();
        let recall_at_k = if query.expected.is_empty() {
            1.0
        } else {
            (query.expected.len() - missing.len()) as f64 / query.expected.len() as f64
        };
        // Rank, not membership: the top-ranked candidate's summary, or the
        // empty string when nothing was returned.
        let top_text = texts.first().cloned().unwrap_or_default();
        let top_hit = query
            .expected
            .iter()
            .any(|marker| top_text.contains(marker.as_str()));

        if outcome.candidates.is_empty() {
            failures.push(Failure::NoCandidates {
                query: query.text.clone(),
            });
        }
        if !missing.is_empty() {
            failures.push(Failure::MissedRecall {
                query: query.text.clone(),
                missing: missing.clone(),
            });
        }
        if !wrong.is_empty() && case.conditions.forbid_wrong_evidence {
            failures.push(Failure::WrongEvidence {
                query: query.text.clone(),
                found: wrong.clone(),
            });
        }
        if !top_hit && case.conditions.expect_top_hit {
            failures.push(Failure::NotTopRanked {
                query: query.text.clone(),
                top: top_text.clone(),
            });
        }

        results.push(QueryResult {
            text: query.text.clone(),
            recall: recall_at_k,
            missing,
            wrong_evidence: wrong,
            returned_ids,
            top_hit,
        });
    }

    samples.sort_unstable();
    let count = results.len().max(1);
    let quality = Quality {
        recall_at_k: results.iter().map(|result| result.recall).sum::<f64>() / count as f64,
        miss_rate: results
            .iter()
            .filter(|result| !result.missing.is_empty())
            .count() as f64
            / count as f64,
        wrong_evidence_rate: results
            .iter()
            .filter(|result| !result.wrong_evidence.is_empty())
            .count() as f64
            / count as f64,
    };
    let latency = Latency {
        samples: samples.len(),
        p50_us: samples.get(samples.len() / 2).copied().unwrap_or(0),
        p95_us: samples
            .get((samples.len() * 95) / 100)
            .copied()
            .unwrap_or(0),
    };

    Ok(CaseResult {
        case_id: case.case_id.clone(),
        snapshot_id: case.snapshot_id.clone(),
        quality,
        cost: Cost {
            embed_calls,
            embedded_texts,
            judge_calls: 0,
            tokens: None,
        },
        latency,
        queries: results,
        failures,
        conditions: case.conditions,
    })
}

/// The aggregate report over a set of cases (stage doc 06 §三轴报告).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub cases: Vec<CaseResult>,
    pub quality: Quality,
    pub cost: Cost,
    pub latency: Latency,
}

impl Report {
    /// Aggregate the per-case results into the three axes.
    pub fn from_cases(cases: Vec<CaseResult>) -> Self {
        let count = cases.len().max(1);
        let quality = Quality {
            recall_at_k: cases
                .iter()
                .map(|case| case.quality.recall_at_k)
                .sum::<f64>()
                / count as f64,
            miss_rate: cases.iter().map(|case| case.quality.miss_rate).sum::<f64>() / count as f64,
            wrong_evidence_rate: cases
                .iter()
                .map(|case| case.quality.wrong_evidence_rate)
                .sum::<f64>()
                / count as f64,
        };
        let cost = Cost {
            embed_calls: cases.iter().map(|case| case.cost.embed_calls).sum(),
            embedded_texts: cases.iter().map(|case| case.cost.embedded_texts).sum(),
            judge_calls: cases.iter().map(|case| case.cost.judge_calls).sum(),
            tokens: None,
        };
        // The aggregate reports the worst per-case p50/p95 rather than
        // re-deriving percentiles across uneven sample counts.
        let latency = Latency {
            samples: cases.iter().map(|case| case.latency.samples).sum(),
            p50_us: cases
                .iter()
                .map(|case| case.latency.p50_us)
                .max()
                .unwrap_or(0),
            p95_us: cases
                .iter()
                .map(|case| case.latency.p95_us)
                .max()
                .unwrap_or(0),
        };
        Self {
            cases,
            quality,
            cost,
            latency,
        }
    }

    /// Whether every case met its frozen conditions.
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(CaseResult::passed)
    }

    /// The manifest-shaped JSON for one case's run, for artifact replay.
    pub fn manifest_json(case: &Case, result: &CaseResult) -> Result<String, String> {
        let manifest = Manifest {
            case: case.clone(),
            run: Run {
                code_revision: code_revision(),
                started_at: stamp(now_seconds()),
                finished_at: stamp(now_seconds()),
                error: if result.failures.is_empty() {
                    None
                } else {
                    Some(format!("{} failure(s)", result.failures.len()))
                },
            },
            result: result.clone(),
        };
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())
    }
}
