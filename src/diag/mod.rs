//! Read-path diagnosis (stage doc 07 S1): attribute a missed recall to the
//! stage that lost it, and summarize a run's arm health.
//!
//! [`attribute`] diagnoses **one run** of [`MemoryRecall::recall`] against a
//! marker string the caller expected back. It replays the retrieval side
//! with the judge stripped — same stores, same arm budget — so the verdict
//! is deterministic and never re-calls a model; the judged run's own state
//! enters as evidence, and the diagnosis never mutates a store.
//!
//! The stage vocabulary is frozen (stage doc 07 §契约): a miss is attributed
//! to exactly one stage, and every verdict carries the typed evidence it was
//! reached from. When the replay and the judged run do not describe the same
//! run — mismatched inputs — the verdict is [`Stage::Indeterminate`], never
//! a guess.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::core::traits::Result;
use crate::core::types::{Fact, FactId, ScopeKey};
use crate::query::facts::{
    ArmName, MemoryRecall, RecallOptions, RecallOutcome, StaleRule, GRAPH_HOPS, JEV_CANDIDATE_CAP,
    SEEDS_PER_ARM,
};

/// Where a recall lost the evidence (stage doc 07 §契约 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    /// The judged run delivered the marker; nothing to attribute.
    Delivered,
    /// No row in the workspace's truth layer carries the marker: the write
    /// path never captured it.
    NotCaptured,
    /// The row exists but is not in the current view (R5): superseded,
    /// `valid_to`-closed, or rejected.
    NotCurrent { reason: NonCurrentReason },
    /// The row exists and is current, but the querying scope does not admit
    /// the scope it was stored under.
    InvisibleToScope,
    /// The row is current and visible, and the caller's staleness rule hid
    /// it; the same replay without the rule returns it.
    StaleFiltered,
    /// No configured arm returned the row, with the arms asked at the
    /// caller's own per-arm budget.
    AllArmsMissed,
    /// An arm returned it, but the retrieval-side order puts it at
    /// `position` ≥ `top_k`.
    RankedOut { position: usize, top_k: usize },
    /// The retrieval-side order had it inside `top_k`; the judgment stage
    /// re-ordered it out.
    DemotedByJudge { pre_rank: usize, top_k: usize },
    /// The judgment stage cleared the entire candidate set (a near-zero
    /// rerank column, or an absent `find` verdict).
    JudgeCleared,
    /// The replay and the judged run do not describe the same run; the
    /// diagnosis refuses to guess.
    Indeterminate,
}

/// Why the row is outside the current view ([`Fact::is_current`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NonCurrentReason {
    Superseded,
    ClosedValidTo,
    Rejected,
}

/// One typed fact behind a verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Evidence {
    /// The workspace-wide truth scan the attribution started from.
    TruthScan { workspace_id: String, rows: usize },
    /// A row whose text contains the marker.
    Captured {
        fact_id: String,
        scope: ScopeKey,
        current: bool,
    },
    /// The visibility verdict's inputs.
    ScopeCheck {
        query: ScopeKey,
        stored: ScopeKey,
        admits: bool,
    },
    /// The caller's staleness rule that hid the row.
    StaleRule {
        max_age_secs: i64,
        min_sessions: usize,
        now_secs: i64,
    },
    /// One arm asked directly, at the caller's per-arm budget. `rank` is the
    /// position in that arm's own ranking when it returned the row; the
    /// graph arm is set-scored, so a touched id reports `Some(0)`.
    ArmProbe {
        arm: ArmName,
        asked: bool,
        rank: Option<usize>,
    },
    /// The row's position in the judge-stripped replay (unbounded `top_k`).
    RetrievalProbe { position: usize },
    /// The caller's `top_k`, the truncation the replay is measured against.
    TopK { top_k: usize },
    /// The judged run's own state, as it entered the diagnosis.
    JudgeRun {
        configured: bool,
        degraded: bool,
        exists: bool,
        candidates: usize,
    },
}

/// The verdict for one marker over one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribution {
    pub stage: Stage,
    pub marker: String,
    pub evidence: Vec<Evidence>,
}

/// Attribute one missed (or delivered) marker.
///
/// `outcome` must be the result of [`MemoryRecall::recall`] called with the
/// same `recall`/`scope`/`query_text`/`query_vec`/`opts`; a run that the
/// replay cannot reproduce is diagnosed as [`Stage::Indeterminate`] rather
/// than mislabelled.
pub async fn attribute(
    recall: &MemoryRecall,
    scope: &ScopeKey,
    query_text: &str,
    query_vec: Option<&[f32]>,
    opts: &RecallOptions,
    marker: &str,
    outcome: &RecallOutcome,
) -> Result<Attribution> {
    // The truth surface is workspace-wide: cross-workspace probing would
    // cross the isolation line the read path exists to keep, so a marker
    // outside this workspace is honestly "not captured".
    let workspace = ScopeKey {
        workspace_id: scope.workspace_id.clone(),
        session_id: None,
        task_id: None,
    };
    let rows = recall.facts.scan(&workspace).await?;
    let captured: Vec<&Fact> = rows
        .iter()
        .filter(|row| row.text.contains(marker))
        .collect();
    let mut evidence = vec![Evidence::TruthScan {
        workspace_id: workspace.workspace_id.clone(),
        rows: rows.len(),
    }];
    let marker = marker.to_string();
    if captured.is_empty() {
        return Ok(Attribution {
            stage: Stage::NotCaptured,
            marker,
            evidence,
        });
    }
    let captured_ids: HashSet<&str> = captured.iter().map(|row| row.id.0.as_str()).collect();
    for row in &captured {
        evidence.push(Evidence::Captured {
            fact_id: row.id.0.clone(),
            scope: row.scope.clone(),
            current: row.is_current(),
        });
    }
    // Delivered is about the judged run's own output: any returned candidate
    // that is one of the marker's rows counts.
    if outcome
        .candidates
        .iter()
        .any(|candidate| captured_ids.contains(candidate.fact_id.0.as_str()))
    {
        return Ok(Attribution {
            stage: Stage::Delivered,
            marker,
            evidence,
        });
    }
    let judge_configured = recall.judge.is_some();
    let judge_degraded = outcome
        .degraded
        .iter()
        .any(|arm| matches!(arm, ArmName::Judge));
    evidence.push(Evidence::JudgeRun {
        configured: judge_configured,
        degraded: judge_degraded,
        exists: outcome.exists,
        candidates: outcome.candidates.len(),
    });
    // The row the attribution follows: the first captured row in the truth
    // scan's stable id order. Every captured row is in the evidence, so a
    // caller can see when the marker matched more than one.
    let row = captured[0];
    if !row.is_current() {
        return Ok(Attribution {
            stage: Stage::NotCurrent {
                reason: non_current_reason(row),
            },
            marker,
            evidence,
        });
    }
    if !scope.admits(&row.scope) {
        evidence.push(Evidence::ScopeCheck {
            query: scope.clone(),
            stored: row.scope.clone(),
            admits: false,
        });
        return Ok(Attribution {
            stage: Stage::InvisibleToScope,
            marker,
            evidence,
        });
    }
    // Judge-stripped replay: same stores, same arm budget, `top_k` lifted so
    // the position is the retrieval-side truth rather than a re-truncation.
    let retrieval = MemoryRecall {
        facts: recall.facts.clone(),
        vector: recall.vector.clone(),
        lexical: recall.lexical.clone(),
        edges: recall.edges.clone(),
        judge: None,
    };
    let fresh = retrieval
        .recall(scope, query_text, query_vec, &probe_options(opts, None))
        .await?;
    let position = fresh
        .candidates
        .iter()
        .position(|candidate| candidate.fact_id == row.id);
    // Staleness is read-time and derived: replay once with the caller's rule
    // to tell "hidden by the rule" from "never retrieved". A row the arms
    // never returned is not "hidden by staleness".
    if let (Some(rule), Some(_)) = (opts.staleness, position) {
        let stale = retrieval
            .recall(
                scope,
                query_text,
                query_vec,
                &probe_options(opts, Some(rule)),
            )
            .await?;
        if !stale
            .candidates
            .iter()
            .any(|candidate| candidate.fact_id == row.id)
        {
            evidence.push(Evidence::StaleRule {
                max_age_secs: rule.max_age_secs,
                min_sessions: rule.min_sessions,
                now_secs: rule.now_secs,
            });
            return Ok(Attribution {
                stage: Stage::StaleFiltered,
                marker,
                evidence,
            });
        }
    }
    let Some(position) = position else {
        evidence.extend(arm_probes(recall, scope, query_text, query_vec, opts, &row.id).await);
        return Ok(Attribution {
            stage: Stage::AllArmsMissed,
            marker,
            evidence,
        });
    };
    evidence.push(Evidence::RetrievalProbe { position });
    evidence.push(Evidence::TopK { top_k: opts.top_k });
    let judge_active = judge_configured && !judge_degraded;
    let stage = if judge_active && !outcome.exists && outcome.candidates.is_empty() {
        Stage::JudgeCleared
    } else if position >= opts.top_k {
        // Even a perfect judgment stage cannot deliver a row the retrieval
        // side never put inside the cut.
        Stage::RankedOut {
            position,
            top_k: opts.top_k,
        }
    } else if judge_active {
        Stage::DemotedByJudge {
            pre_rank: position,
            top_k: opts.top_k,
        }
    } else {
        // The replay would have delivered it and nothing can remove a
        // candidate without the judge: the inputs disagree.
        Stage::Indeterminate
    };
    Ok(Attribution {
        stage,
        marker,
        evidence,
    })
}

fn non_current_reason(row: &Fact) -> NonCurrentReason {
    if row.superseded_by.is_some() {
        NonCurrentReason::Superseded
    } else if row.valid_to.is_some() {
        NonCurrentReason::ClosedValidTo
    } else {
        NonCurrentReason::Rejected
    }
}

/// The replay options: same arm budget and damping as the caller, `top_k`
/// unbounded — the replay's job is to expose the cut position, not to re-cut
/// it — and staleness exactly as told.
fn probe_options(opts: &RecallOptions, staleness: Option<StaleRule>) -> RecallOptions {
    RecallOptions {
        top_k: usize::MAX,
        rrf_k: opts.rrf_k,
        per_arm_limit: opts.per_arm_limit,
        staleness,
    }
}

/// Direct arm probes at the caller's own per-arm budget: which arms were
/// asked, and which returned the row. Errors inside a probe are absorbed —
/// the arm's own failure is already reported by the run's `degraded` list,
/// and a diagnosis never fails on a broken arm.
async fn arm_probes(
    recall: &MemoryRecall,
    scope: &ScopeKey,
    query_text: &str,
    query_vec: Option<&[f32]>,
    opts: &RecallOptions,
    target: &FactId,
) -> Vec<Evidence> {
    let limit = opts.per_arm_limit.min(JEV_CANDIDATE_CAP);
    let mut evidence = Vec::new();

    let (vector_hits, vector_asked) = match (&recall.vector, query_vec) {
        (Some(store), Some(query)) => (
            store.search(scope, query, limit).await.unwrap_or_default(),
            true,
        ),
        _ => (Vec::new(), false),
    };
    evidence.push(Evidence::ArmProbe {
        arm: ArmName::Vector,
        asked: vector_asked,
        rank: rank_of(&vector_hits, target),
    });

    let (lexical_hits, lexical_asked) = match &recall.lexical {
        Some(store) => (
            store
                .search(scope, query_text, limit)
                .await
                .unwrap_or_default(),
            true,
        ),
        None => (Vec::new(), false),
    };
    evidence.push(Evidence::ArmProbe {
        arm: ArmName::Lexical,
        asked: lexical_asked,
        rank: rank_of(&lexical_hits, target),
    });

    // The graph arm seeds from the top [`SEEDS_PER_ARM`] of each hit arm, in
    // the same id-sorted order the recall uses, so the probe asks the same
    // neighborhood.
    let mut rank = None;
    let graph_asked = recall.edges.is_some();
    if let Some(store) = &recall.edges {
        let mut seeds: HashSet<FactId> = HashSet::new();
        for hit in vector_hits.iter().take(SEEDS_PER_ARM) {
            seeds.insert(hit.0.clone());
        }
        for hit in lexical_hits.iter().take(SEEDS_PER_ARM) {
            seeds.insert(hit.0.clone());
        }
        let mut seed_list: Vec<FactId> = seeds.into_iter().collect();
        seed_list.sort_by(|left, right| left.0.cmp(&right.0));
        if let Ok(edges) = store.neighbors(&seed_list, GRAPH_HOPS, scope).await {
            if edges
                .iter()
                .any(|edge| &edge.src == target || &edge.dst == target)
            {
                rank = Some(0);
            }
        }
    }
    evidence.push(Evidence::ArmProbe {
        arm: ArmName::Graph,
        asked: graph_asked,
        rank,
    });
    evidence
}

fn rank_of(hits: &[(FactId, f64)], target: &FactId) -> Option<usize> {
    hits.iter().position(|(id, _)| id == target)
}

/// One arm's supply in a run's candidate set: how many candidates it
/// contributed, and how many of those no other arm found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmSupply {
    pub arm: String,
    pub candidates: usize,
    pub exclusive: usize,
}

/// The health row for one recall (stage doc 07 S1 D2): what the run
/// returned, whether `top_k` cut it short, which arms degraded, and what each
/// arm contributed — derived from the outcome alone, no store reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthRow {
    pub exists: bool,
    pub candidates: usize,
    /// `candidates == top_k`: more may lie below the cut.
    pub saturated: bool,
    pub degraded: Vec<String>,
    pub supplies: Vec<ArmSupply>,
}

/// Summarize one run. `top_k` is the caller's budget, needed only for the
/// saturation flag: `top_k == 0` can never be saturated.
pub fn health(outcome: &RecallOutcome, top_k: usize) -> HealthRow {
    let supplies = [ArmName::Vector, ArmName::Lexical, ArmName::Graph]
        .into_iter()
        .map(|arm| ArmSupply {
            arm: arm.to_string(),
            candidates: outcome
                .candidates
                .iter()
                .filter(|candidate| candidate.via.contains(&arm))
                .count(),
            exclusive: outcome
                .candidates
                .iter()
                .filter(|candidate| candidate.via.len() == 1 && candidate.via[0] == arm)
                .count(),
        })
        .collect();
    HealthRow {
        exists: outcome.exists,
        candidates: outcome.candidates.len(),
        saturated: top_k > 0 && outcome.candidates.len() == top_k,
        degraded: outcome.degraded.iter().map(ToString::to_string).collect(),
        supplies,
    }
}
