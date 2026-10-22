//! Miss attribution contract (stage doc 07 S1): every `Stage` the frozen
//! vocabulary names, reached from a real `recall` outcome, plus the health
//! row. Each test states the bug that turns it red — the diagnosis is a
//! read-path output, so its own failure modes are the subject here.
//!
//! Fixtures: `MemoryFactStore` (the in-memory truth layer) plus fixed
//! lexical rankings for the label tests — the arms' own ranking contracts
//! are tested by `tests/lexical_recall.rs`; the diagnosis reasons about a
//! deterministic *result*, and one test at the end runs the real Turso
//! backends end to end.

use std::sync::Arc;

use async_trait::async_trait;
use mnemo2u::core::traits::{FactStore, LexicalStore, Result as StoreResult};
use mnemo2u::core::types::{Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::diag::{attribute, health, Evidence, NonCurrentReason, Stage};
use mnemo2u::eval::open_stores;
use mnemo2u::llm::judge::ExistsVerdict;
use mnemo2u::llm::judge_mock::JudgeMock;
use mnemo2u::query::facts::{
    ArmName, L0Candidate, MemoryRecall, RecallOptions, RecallOutcome, StaleRule,
};
use mnemo2u::store::facts::MemoryFactStore;
use mnemo2u::store::turso_lexical::TursoLexical;

fn scope(ws: &str, sess: Option<&str>, task: Option<&str>) -> ScopeKey {
    ScopeKey {
        workspace_id: ws.to_string(),
        session_id: sess.map(str::to_string),
        task_id: task.map(str::to_string),
    }
}

fn id(value: &str) -> FactId {
    FactId(value.to_string())
}

fn source_ref(session: &str) -> SourceRef {
    SourceRef {
        workspace_id: "ws-a".to_string(),
        session_id: session.to_string(),
        branch_id: "main".to_string(),
        epoch: 7,
        event_id: "ev-1".to_string(),
    }
}

/// One current, single-session fact as the arms would have captured it.
fn fact(fact_id: &str, scope: &ScopeKey, text: &str) -> Fact {
    Fact {
        id: id(fact_id),
        scope: scope.clone(),
        kind: "semantic".to_string(),
        text: text.to_string(),
        source_refs: vec![source_ref("s1")],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: 42_000,
        superseded_by: None,
        revision: 0,
    }
}

fn opts(top_k: usize, staleness: Option<StaleRule>) -> RecallOptions {
    RecallOptions {
        top_k,
        rrf_k: 60,
        per_arm_limit: 50,
        staleness,
    }
}

/// A lexical arm with a fixed ranking: the diagnosis only needs a
/// deterministic result to reason about.
struct FixedLexical {
    ranking: Vec<(FactId, f64)>,
}

#[async_trait]
impl LexicalStore for FixedLexical {
    async fn index(&self, _scope: &ScopeKey, _id: &FactId, _text: &str) -> StoreResult<()> {
        Ok(())
    }

    async fn drop(&self, _scope: &ScopeKey, _id: &FactId) -> StoreResult<()> {
        Ok(())
    }

    async fn search(
        &self,
        _scope: &ScopeKey,
        _query: &str,
        top_k: usize,
    ) -> StoreResult<Vec<(FactId, f64)>> {
        Ok(self.ranking.iter().take(top_k).cloned().collect())
    }
}

fn recall_with(facts: Arc<MemoryFactStore>, ranking: Vec<(&str, f64)>) -> MemoryRecall {
    MemoryRecall {
        facts,
        vector: None,
        lexical: Some(Arc::new(FixedLexical {
            ranking: ranking
                .into_iter()
                .map(|(fact_id, score)| (id(fact_id), score))
                .collect(),
        })),
        edges: None,
        judge: None,
    }
}

/// Delivered: the judged run already returned the marker's row — the
/// diagnosis must not attribute anything. Red on: a delivery check that
/// matches summaries instead of ids (the marker sits past the 200-char
/// summary cut), or one that runs after the scan and reports a later stage.
#[tokio::test]
async fn delivered_marker_reports_delivered() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    let text = format!("{} the quartz ledger holds beta", "filler ".repeat(40));
    facts.put(&fact("f-hit", &ws, &text)).await.expect("put");
    let recall = recall_with(facts, vec![("f-hit", 1.0)]);
    let options = opts(5, None);
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::Delivered);
    assert!(matches!(
        attribution.evidence[0],
        Evidence::TruthScan { .. }
    ));
}

/// NotCaptured: nothing in the workspace truth carries the marker. Red on:
/// scanning the querying scope instead of the workspace one (a session
/// scope would drop the row and turn this into a false verdict), or a scan
/// that never ran.
#[tokio::test]
async fn uncaptured_marker_reports_not_captured() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", Some("s1"), None);
    facts
        .put(&fact("f-other", &ws, "unrelated row"))
        .await
        .expect("put");
    let recall = recall_with(facts, vec![]);
    let options = opts(5, None);
    let outcome = recall
        .recall(&ws, "harbour ledger", None, &options)
        .await
        .expect("recall");
    let attribution = attribute(
        &recall,
        &ws,
        "harbour ledger",
        None,
        &options,
        "harbour ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::NotCaptured);
    assert!(attribution
        .evidence
        .iter()
        .any(|item| matches!(item, Evidence::TruthScan { rows: 1, .. })));
}

/// NotCurrent{Superseded}: the row exists but a correction superseded it —
/// the read-side half of R5. Red on: an attribution that checks
/// `is_current` after the scope admission (it would report a different
/// stage), or one that lets a superseded row through the captured check.
#[tokio::test]
async fn superseded_marker_reports_not_current() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    let mut old = fact("f-old", &ws, "the quartz ledger holds beta");
    old.superseded_by = Some(id("f-new"));
    facts.put(&old).await.expect("put old");
    facts
        .put(&fact("f-new", &ws, "the replacement row"))
        .await
        .expect("put new");
    let recall = recall_with(facts, vec![("f-old", 1.0)]);
    let options = opts(5, None);
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    assert!(outcome.candidates.is_empty(), "a superseded row reached L0");
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(
        attribution.stage,
        Stage::NotCurrent {
            reason: NonCurrentReason::Superseded
        }
    );
    assert!(attribution
        .evidence
        .iter()
        .any(|item| matches!(item, Evidence::Captured { current: false, .. })));
}

/// InvisibleToScope: the row is stored under session s2 while the query runs
/// under s1. The real Turso FTS keeps its scope filter *inside* the query
/// (stage doc 03 contract 1), so the miss is genuine; the workspace scan
/// still finds the row, and admission must run query.admits(stored) — not
/// the reverse. Red on: a flipped `admits`, or a scan scoped to the query.
#[tokio::test]
async fn cross_session_marker_reports_invisible_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = Arc::new(MemoryFactStore::new());
    let stored = scope("ws-a", Some("s2"), None);
    let query = scope("ws-a", Some("s1"), None);
    let text = "the kelpite shelf holds the staged notes";
    facts.put(&fact("f-s2", &stored, text)).await.expect("put");
    let lexical = TursoLexical::new(&dir.path().join("lexical.db").to_string_lossy())
        .await
        .expect("lexical");
    lexical
        .index(&stored, &id("f-s2"), text)
        .await
        .expect("index");
    let recall = MemoryRecall {
        facts: facts.clone(),
        vector: None,
        lexical: Some(Arc::new(lexical)),
        edges: None,
        judge: None,
    };
    let options = opts(5, None);
    let outcome = recall
        .recall(&query, "kelpite shelf", None, &options)
        .await
        .expect("recall");
    assert!(outcome.candidates.is_empty(), "cross-session row leaked");
    let attribution = attribute(
        &recall,
        &query,
        "kelpite shelf",
        None,
        &options,
        "kelpite shelf",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::InvisibleToScope);
    assert!(attribution
        .evidence
        .iter()
        .any(|item| matches!(item, Evidence::ScopeCheck { admits: false, .. })));
}

/// StaleFiltered: the caller's read-time rule hides an old, under-
/// corroborated row; the judge-stripped replay without the rule returns it,
/// so "hidden by the rule" is told apart from "never retrieved". Red on: a
/// replay that keeps the caller's staleness (the row then vanishes and the
/// verdict falls through), or a rule probe that never flips it.
#[tokio::test]
async fn stale_hidden_marker_reports_stale_filtered() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    facts
        .put(&fact("f-old", &ws, "the quartz ledger holds beta"))
        .await
        .expect("put");
    let recall = recall_with(facts, vec![("f-old", 1.0)]);
    let rule = StaleRule {
        max_age_secs: 10_000,
        min_sessions: 2,
        now_secs: 100_000,
    };
    let options = opts(5, Some(rule));
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    assert!(
        outcome.candidates.is_empty(),
        "the rule did not hide the row"
    );
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::StaleFiltered);
    assert!(attribution.evidence.iter().any(|item| matches!(
        item,
        Evidence::StaleRule {
            max_age_secs: 10_000,
            ..
        }
    )));
}

/// AllArmsMissed names the probes: lexical asked and empty, vector not asked
/// (no query vector), graph not configured. Red on: `ArmProbe` flags taken
/// from the wrong store handles, an `asked` flag that reads "configured"
/// where no vector was supplied, or a verdict with no probe evidence.
#[tokio::test]
async fn all_arms_missed_names_the_probes() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    facts
        .put(&fact("f-a", &ws, "the quartz ledger holds beta"))
        .await
        .expect("put");
    let recall = recall_with(facts, vec![]);
    let options = opts(5, None);
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::AllArmsMissed);
    assert!(attribution.evidence.iter().any(|item| matches!(
        item,
        Evidence::ArmProbe {
            arm: ArmName::Lexical,
            asked: true,
            rank: None
        }
    )));
    assert!(attribution.evidence.iter().any(|item| matches!(
        item,
        Evidence::ArmProbe {
            arm: ArmName::Vector,
            asked: false,
            rank: None
        }
    )));
    assert!(attribution.evidence.iter().any(|item| matches!(
        item,
        Evidence::ArmProbe {
            arm: ArmName::Graph,
            asked: false,
            rank: None
        }
    )));
}

/// RankedOut{position,top_k}: the arm returned the row, but the
/// retrieval-side order puts it at 1 while `top_k` is 1. Red on: computing
/// the position after truncation (the row is gone and the verdict falls to
/// Indeterminate), or an off-by-one at the cut (`position <= top_k`).
#[tokio::test]
async fn ranked_out_reports_the_cut_position() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    facts
        .put(&fact("f-a", &ws, "the quartz ledger holds beta"))
        .await
        .expect("put a");
    facts
        .put(&fact("f-b", &ws, "the quartz layout is unrelated"))
        .await
        .expect("put b");
    let recall = recall_with(facts, vec![("f-b", 2.0), ("f-a", 1.0)]);
    let options = opts(1, None);
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    assert_eq!(outcome.candidates.len(), 1);
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(
        attribution.stage,
        Stage::RankedOut {
            position: 1,
            top_k: 1
        }
    );
    assert!(attribution
        .evidence
        .iter()
        .any(|item| matches!(item, Evidence::RetrievalProbe { position: 1 })));
}

/// DemotedByJudge{pre_rank,top_k}: the retrieval side had the row first; the
/// scripted reranker put the other candidate above it and `top_k` cuts the
/// row out. Red on: a replay that keeps the judge configured (it would
/// reproduce the demotion and report RankedOut), or a branch that reports
/// RankedOut whenever the judged run lacks the row.
#[tokio::test]
async fn demoted_by_judge_reports_pre_rank() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    facts
        .put(&fact("f-a", &ws, "the quartz ledger holds beta"))
        .await
        .expect("put a");
    facts
        .put(&fact("f-b", &ws, "the quartz layout is unrelated"))
        .await
        .expect("put b");
    let mut recall = recall_with(facts, vec![("f-a", 1.0), ("f-b", 0.5)]);
    recall.judge = Some(Arc::new(JudgeMock::new(
        "judge-mock",
        vec![
            JudgeMock::rerank_ranked(&[("f-b", 0.9), ("f-a", 0.4)]),
            JudgeMock::find(Some(("f-b", 0.9)), ExistsVerdict::Answered),
        ],
    )));
    let options = opts(1, None);
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    assert!(outcome.exists);
    assert_eq!(outcome.candidates.len(), 1);
    assert_eq!(outcome.candidates[0].fact_id, id("f-b"));
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(
        attribution.stage,
        Stage::DemotedByJudge {
            pre_rank: 0,
            top_k: 1
        }
    );
}

/// JudgeCleared: a near-zero rerank column empties the set. Red on: ordering
/// the cleared branch after the position branches (with `top_k = 5` the
/// verdict would claim a demotion the cut never made), or reading
/// `degraded` to mean "the judge answered".
#[tokio::test]
async fn judge_cleared_reports_cleared() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    facts
        .put(&fact("f-a", &ws, "the quartz ledger holds beta"))
        .await
        .expect("put a");
    facts
        .put(&fact("f-b", &ws, "the quartz layout is unrelated"))
        .await
        .expect("put b");
    let mut recall = recall_with(facts, vec![("f-a", 1.0), ("f-b", 0.5)]);
    recall.judge = Some(Arc::new(JudgeMock::new(
        "judge-mock",
        vec![JudgeMock::rerank_ranked(&[("f-a", 0.0), ("f-b", 0.02)])],
    )));
    let options = opts(5, None);
    let outcome = recall
        .recall(&ws, "quartz", None, &options)
        .await
        .expect("recall");
    assert!(!outcome.exists);
    assert!(outcome.candidates.is_empty());
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::JudgeCleared);
    assert!(attribution.evidence.iter().any(|item| matches!(
        item,
        Evidence::JudgeRun {
            configured: true,
            degraded: false,
            exists: false,
            candidates: 0
        }
    )));
}

/// Indeterminate: the replay says the run should have delivered the row —
/// the `outcome` under diagnosis belongs to a different run. Red on: any
/// code path that fills in a plausible-looking stage instead of admitting
/// the mismatch.
#[tokio::test]
async fn indeterminate_when_outcome_does_not_match() {
    let facts = Arc::new(MemoryFactStore::new());
    let ws = scope("ws-a", None, None);
    facts
        .put(&fact("f-a", &ws, "the quartz ledger holds beta"))
        .await
        .expect("put");
    let recall = recall_with(facts, vec![("f-a", 1.0)]);
    let options = opts(5, None);
    let bogus = RecallOutcome {
        exists: false,
        candidates: Vec::new(),
        degraded: Vec::new(),
    };
    let attribution = attribute(
        &recall,
        &ws,
        "quartz",
        None,
        &options,
        "quartz ledger",
        &bogus,
    )
    .await
    .expect("attribute");
    assert_eq!(attribution.stage, Stage::Indeterminate);
}

/// HealthRow counts per-arm supply and exclusivity and marks a saturated
/// cut. Red on: `exclusive` computed as "via is longer than one", a supply
/// counted from `degraded` instead of `via`, or a saturation flag that
/// ignores `top_k == 0`.
#[test]
fn health_counts_supplies_and_saturation() {
    let candidate = |fact_id: &str, via: Vec<ArmName>| L0Candidate {
        fact_id: id(fact_id),
        summary: "summary".to_string(),
        source_refs: Vec::new(),
        trust_state: TrustState::Verified,
        score: 1.0,
        via,
    };
    let outcome = RecallOutcome {
        exists: true,
        candidates: vec![
            candidate("f-1", vec![ArmName::Lexical]),
            candidate("f-2", vec![ArmName::Vector, ArmName::Lexical]),
        ],
        degraded: vec![ArmName::Graph],
    };
    let row = health(&outcome, 2);
    assert!(row.exists);
    assert!(row.saturated);
    assert_eq!(row.degraded, vec!["graph".to_string()]);
    assert_eq!(row.supplies[0].arm, "vector");
    assert_eq!(row.supplies[0].candidates, 1);
    assert_eq!(row.supplies[0].exclusive, 0);
    assert_eq!(row.supplies[1].arm, "lexical");
    assert_eq!(row.supplies[1].candidates, 2);
    assert_eq!(row.supplies[1].exclusive, 1);
    assert_eq!(row.supplies[2].candidates, 0);
    assert!(!health(&outcome, 0).saturated);
}

/// Real backends: the durable Turso stores the memory layer runs on. A
/// captured row is delivered; an unwritten marker is NotCaptured — the
/// terminals hold end to end, not just against fixed rankings. Red on: an
/// attribution that needs the fixed ranking's shape to work at all, or a
/// truth scan that misses durable rows.
#[tokio::test]
async fn real_backends_deliver_and_report_uncaptured() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = open_stores(dir.path()).await.expect("stores");
    let ws = scope("ws-a", None, None);
    let text = "the quartz ledger holds the drift numbers";
    stores
        .facts
        .put(&fact("f-quartz", &ws, text))
        .await
        .expect("put");
    stores
        .lexical
        .index(&ws, &id("f-quartz"), text)
        .await
        .expect("index");
    let recall = MemoryRecall {
        facts: stores.facts.clone(),
        vector: None,
        lexical: Some(stores.lexical.clone()),
        edges: Some(stores.edges.clone()),
        judge: None,
    };
    let options = opts(5, None);
    let outcome = recall
        .recall(&ws, "quartz ledger", None, &options)
        .await
        .expect("recall");
    let delivered = attribute(
        &recall,
        &ws,
        "quartz ledger",
        None,
        &options,
        "quartz ledger",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(delivered.stage, Stage::Delivered);
    let uncaptured = attribute(
        &recall,
        &ws,
        "quartz ledger",
        None,
        &options,
        "never written marker",
        &outcome,
    )
    .await
    .expect("attribute");
    assert_eq!(uncaptured.stage, Stage::NotCaptured);
}
