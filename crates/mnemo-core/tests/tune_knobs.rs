//! The tuning surface (stage doc 08 S2): the registry mirrors the constants,
//! the stage mapping is the frozen one, and the default assembly is
//! unchanged.
//!
//! Each test names the bug it turns red.

use std::path::PathBuf;

use mnemo2u::core::traits::{FactEdgeStore, FactStore, LexicalStore};
use mnemo2u::core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
use mnemo2u::diag::{NonCurrentReason, Stage};
use mnemo2u::eval::{cases, run_case};
use mnemo2u::query::facts::{MemoryRecall, RecallKnobs, RecallOptions, RecallOutcome};
use mnemo2u::tune::{self, KnobLayer, KnobValue};

const QUERY: &str = "chrysalis ledger rotation policy";

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnemo2u-tune-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The ids a stage maps to, in registry order.
fn knob_ids(knobs: Vec<&'static tune::Knob>) -> Vec<&'static str> {
    knobs.into_iter().map(|knob| knob.id).collect()
}

/// The rows an outcome returned, in rank order.
fn fact_ids(outcome: &RecallOutcome) -> Vec<String> {
    outcome
        .candidates
        .iter()
        .map(|candidate| candidate.fact_id.0.clone())
        .collect()
}

/// Everything a caller can observe of one candidate, in rank order.
fn fingerprint(outcome: &RecallOutcome) -> Vec<(String, String, f64, Vec<String>)> {
    outcome
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.fact_id.0.clone(),
                candidate.summary.clone(),
                candidate.score,
                candidate.via.iter().map(ToString::to_string).collect(),
            )
        })
        .collect()
}

/// One fact row in the fixture scope. `text` is long enough that the default
/// summary budget has something to cut.
fn fact(scope: &ScopeKey, id: &str, text: &str) -> Fact {
    Fact {
        id: FactId(id.to_string()),
        scope: scope.clone(),
        kind: "semantic".to_string(),
        text: text.to_string(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: "s1".to_string(),
            branch_id: "main".to_string(),
            epoch: 1,
            event_id: format!("ev-{id}"),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: 1,
        superseded_by: None,
        revision: 0,
    }
}

/// A durable store set with two rows: the seed matches the query lexically,
/// and the second is reachable only through a `Corrects` edge — so the graph
/// arm, its seeds and the rel weights are all on the path under test.
async fn fixture(name: &str) -> (MemoryRecall, ScopeKey) {
    let stores = mnemo2u::eval::open_stores(&workspace(name))
        .await
        .expect("stores open");
    let scope = ScopeKey {
        workspace_id: "tune-ws".to_string(),
        session_id: None,
        task_id: None,
    };
    let seed = format!("{QUERY} {}", "filler ".repeat(60));
    let linked = "the vault secret manager holds the rotated value".to_string();
    stores
        .facts
        .put(&fact(&scope, "f-seed", &seed))
        .await
        .expect("seed row");
    stores
        .facts
        .put(&fact(&scope, "f-linked", &linked))
        .await
        .expect("linked row");
    stores
        .lexical
        .index(&scope, &FactId("f-seed".to_string()), &seed)
        .await
        .expect("seed index");
    stores
        .lexical
        .index(&scope, &FactId("f-linked".to_string()), &linked)
        .await
        .expect("linked index");
    stores
        .edges
        .upsert_edges(
            &scope,
            &[Edge {
                src: FactId("f-seed".to_string()),
                dst: FactId("f-linked".to_string()),
                rel: RelKind::Corrects,
                weight: 1.0,
                source_event_id: "ev-1".to_string(),
            }],
        )
        .await
        .expect("edge");
    let recall = MemoryRecall {
        facts: stores.facts.clone(),
        vector: None,
        lexical: Some(stores.lexical.clone()),
        edges: Some(stores.edges.clone()),
        judge: None,
    };
    (recall, scope)
}

/// Bug this turns red: a knob constant moved in `src/query/facts.rs` without
/// the registry row that documents it — or a registry row edited to a value
/// the code no longer uses. The literals below are the pinned copy; the
/// comparison against `RecallKnobs::default()` is the live source.
#[test]
fn defaults_match_current_values() {
    let registry = tune::default_registry();
    let pinned = [
        ("recall.graph_hops", 2.0),
        ("recall.jev_candidate_cap", 250.0),
        ("recall.rerank_near_zero", 0.05),
        ("recall.rel_correction", 1.5),
        ("recall.rel_support", 1.2),
        ("recall.seeds_per_arm", 10.0),
        ("recall.summary_chars", 200.0),
        // The damping is the one per-call budget with a single value across
        // the tree, so it is pinned too.
        ("recall.rrf_k", 60.0),
    ];
    for (id, value) in pinned {
        let knob = registry.get(id).expect("a registered knob");
        assert_eq!(
            knob.current.as_f64(),
            Some(value),
            "registry row `{id}` drifted from the value the default assembly runs"
        );
    }

    let live = RecallKnobs::default();
    assert_eq!(
        registry.get("recall.graph_hops").unwrap().current,
        KnobValue::U8(live.graph_hops)
    );
    assert_eq!(
        registry.get("recall.jev_candidate_cap").unwrap().current,
        KnobValue::Usize(live.jev_candidate_cap)
    );
    assert_eq!(
        registry.get("recall.seeds_per_arm").unwrap().current,
        KnobValue::Usize(live.seeds_per_arm)
    );
    assert_eq!(
        registry.get("recall.summary_chars").unwrap().current,
        KnobValue::Usize(live.summary_chars)
    );
    assert_eq!(
        registry.get("recall.rerank_near_zero").unwrap().current,
        KnobValue::F64(live.rerank_near_zero)
    );
    assert_eq!(
        registry.get("recall.rel_correction").unwrap().current,
        KnobValue::F64(live.rel_correction)
    );
    assert_eq!(
        registry.get("recall.rel_support").unwrap().current,
        KnobValue::F64(live.rel_support)
    );

    // The bridge a tuning pass crosses: registry values back into the struct
    // the read path takes must land on the default assembly exactly.
    assert_eq!(
        registry.recall_knobs().expect("the seven are set"),
        live,
        "the registry must inject the default assembly, not a near copy of it"
    );

    // The per-call budgets have no value in the default assembly: they are
    // recorded as unset rather than guessed.
    for id in [
        "recall.per_arm_limit",
        "recall.stale.max_age_secs",
        "recall.stale.min_sessions",
        "recall.top_k",
    ] {
        assert_eq!(
            registry.get(id).expect("a registered knob").current,
            KnobValue::Unset,
            "`{id}` is the caller's per call and must not be recorded as a fixed value"
        );
    }
}

/// Bug this turns red: a stage losing its mapping, or gaining one it must not
/// have — `InvisibleToScope` is a scope policy and `NotCaptured` is the write
/// path, so a knob there would be a plausible-looking guess.
#[test]
fn stage_maps_to_its_knobs() {
    let arms = [
        "recall.graph_hops",
        "recall.jev_candidate_cap",
        "recall.per_arm_limit",
        "recall.rel_correction",
        "recall.rel_support",
        "recall.seeds_per_arm",
    ];
    let mut ranked_out = arms.to_vec();
    ranked_out.extend(["recall.rrf_k", "recall.top_k"]);
    ranked_out.sort_unstable();

    let cases: [(Stage, &[&str]); 12] = [
        (Stage::Delivered, &["recall.summary_chars"]),
        (Stage::NotCaptured, &[]),
        (
            Stage::NotCurrent {
                reason: NonCurrentReason::Superseded,
            },
            &[],
        ),
        (
            Stage::NotCurrent {
                reason: NonCurrentReason::ClosedValidTo,
            },
            &[],
        ),
        (
            Stage::NotCurrent {
                reason: NonCurrentReason::Rejected,
            },
            &[],
        ),
        (Stage::InvisibleToScope, &[]),
        (
            Stage::StaleFiltered,
            &["recall.stale.max_age_secs", "recall.stale.min_sessions"],
        ),
        (Stage::AllArmsMissed, &arms),
        (
            Stage::RankedOut {
                position: 7,
                top_k: 5,
            },
            &ranked_out,
        ),
        (
            Stage::DemotedByJudge {
                pre_rank: 1,
                top_k: 5,
            },
            &["recall.rerank_near_zero"],
        ),
        (Stage::JudgeCleared, &["recall.rerank_near_zero"]),
        (Stage::Indeterminate, &[]),
    ];
    for (stage, expected) in cases {
        assert_eq!(
            knob_ids(tune::knobs_for(&stage)),
            expected,
            "the knob set for {stage:?} drifted"
        );
    }

    // The layers themselves, so a layer added to a stage is red even when it
    // happens to select the same knobs today.
    assert_eq!(tune::layers_for(&Stage::Delivered), &[KnobLayer::Delivery]);
    assert!(tune::layers_for(&Stage::InvisibleToScope).is_empty());
    assert_eq!(
        tune::layers_for(&Stage::StaleFiltered),
        &[KnobLayer::Staleness]
    );
    assert_eq!(tune::layers_for(&Stage::AllArmsMissed), &[KnobLayer::Arms]);
    assert_eq!(
        tune::layers_for(&Stage::RankedOut {
            position: 0,
            top_k: 1
        }),
        &[KnobLayer::Fusion, KnobLayer::Arms]
    );
    assert_eq!(tune::layers_for(&Stage::JudgeCleared), &[KnobLayer::Judge]);
    assert!(tune::layers_for(&Stage::Indeterminate).is_empty());
}

/// Bug this turns red: the surface growing a knob no diagnosis can reach —
/// the stage mapping and the registry have to describe the same thing.
#[test]
fn every_knob_is_reachable_from_a_stage() {
    let stages = [
        Stage::Delivered,
        Stage::NotCaptured,
        Stage::NotCurrent {
            reason: NonCurrentReason::Superseded,
        },
        Stage::InvisibleToScope,
        Stage::StaleFiltered,
        Stage::AllArmsMissed,
        Stage::RankedOut {
            position: 1,
            top_k: 5,
        },
        Stage::DemotedByJudge {
            pre_rank: 0,
            top_k: 5,
        },
        Stage::JudgeCleared,
        Stage::Indeterminate,
    ];
    let mut reachable: Vec<&str> = stages
        .iter()
        .flat_map(|stage| knob_ids(tune::knobs_for(stage)))
        .collect();
    reachable.sort_unstable();
    reachable.dedup();

    let mut registered: Vec<&str> = tune::default_registry()
        .knobs()
        .iter()
        .map(|knob| knob.id)
        .collect();
    registered.sort_unstable();
    assert!(!registered.is_empty(), "the registry is empty");
    assert_eq!(
        reachable, registered,
        "every registered knob must be reachable from some stage, and no stage may name a knob that is not registered"
    );

    // A stage also must not name the same knob twice.
    for stage in stages {
        let ids = knob_ids(tune::knobs_for(&stage));
        let mut once = ids.clone();
        once.dedup();
        assert_eq!(ids, once, "{stage:?} names a knob twice");
    }
}

/// Bug this turns red: a boundary mis-declared — a row whose own value is
/// outside its window, or a per-arm window that no longer matches the cap it
/// is clamped against.
#[test]
fn knob_values_fit_their_bounds() {
    let registry = tune::default_registry();
    for knob in registry.knobs() {
        assert!(
            knob.bounds.admits(&knob.current),
            "`{}` ({:?}) is outside the window the registry states ({:?})",
            knob.id,
            knob.current,
            knob.bounds
        );
        assert!(!knob.rationale.is_empty(), "`{}` has no rationale", knob.id);
    }
    let cap = registry.get("recall.jev_candidate_cap").expect("cap");
    let per_arm = registry.get("recall.per_arm_limit").expect("per-arm");
    assert_eq!(
        per_arm.bounds.max,
        cap.current.as_f64(),
        "a per-arm budget above the cap is clamped to it, so the window must stop at the cap"
    );
}

/// Bug this turns red: an unknown knob name panicking or passing silently,
/// where a tuning pass needs to be told its step named nothing.
#[test]
fn unknown_knob_id_is_an_error() {
    let registry = tune::default_registry();
    let error = registry
        .get("recall.nope")
        .expect_err("an unknown id is an error, not a panic and not a value");
    assert!(matches!(error, tune::TuneError::UnknownKnob { .. }));
    assert!(error.to_string().contains("recall.nope"));
}

/// Bug this turns red: a row reordered or a serialized field changed — which
/// would make every committed baseline diff unreadable, and make "the same
/// registry" and "the same bytes" stop meaning the same thing.
#[test]
fn registry_serializes_stably() {
    let registry = tune::default_registry();
    let first = registry.json();
    assert_eq!(
        first,
        tune::default_registry().json(),
        "the same registry must serialize to the same bytes"
    );

    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&first).expect("the registry is a JSON array");
    assert_eq!(rows.len(), 12, "the registry grew or shrank");
    let ids: Vec<String> = rows
        .iter()
        .map(|row| row["id"].as_str().expect("every row has an id").to_string())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(ids, sorted, "rows must be in id order, with no duplicates");
    for row in &rows {
        assert!(row["layer"].is_string(), "every row has a layer: {row}");
        assert!(row["current"].is_object(), "every row has a value: {row}");
        assert!(row["bounds"].is_object(), "every row has bounds: {row}");
        assert!(row["rationale"].is_string(), "every row has a rationale");
    }
}

/// Bug this turns red: the read path injecting something that is not the
/// default assembly — this is the zero-behaviour-change check, and it fails
/// on any difference a caller could observe.
#[tokio::test]
async fn recall_with_defaults_is_the_recall_path() {
    let (recall, scope) = fixture("defaults-path").await;
    let options = RecallOptions {
        top_k: 10,
        rrf_k: 60,
        per_arm_limit: 50,
        staleness: None,
    };
    let default_path = recall
        .recall(&scope, QUERY, None, &options)
        .await
        .expect("recall runs");
    let injected = recall
        .recall_with(&scope, QUERY, None, &options, &RecallKnobs::default())
        .await
        .expect("recall runs");
    assert_eq!(default_path.exists, injected.exists);
    assert_eq!(default_path.degraded, injected.degraded);
    assert_eq!(
        fingerprint(&default_path),
        fingerprint(&injected),
        "the injected-with-defaults path must be the same run, candidate for candidate"
    );
    assert_eq!(
        fact_ids(&default_path).len(),
        2,
        "the fixture must exercise both the lexical and the graph arm"
    );
}

/// Bug this turns red: an injection seam that is wired to nothing — a moved
/// knob that leaves the run unchanged.
#[tokio::test]
async fn a_moved_knob_reaches_the_read_path() {
    let (recall, scope) = fixture("moved-knob").await;
    let options = RecallOptions {
        top_k: 10,
        rrf_k: 60,
        per_arm_limit: 50,
        staleness: None,
    };
    let default_path = recall
        .recall(&scope, QUERY, None, &options)
        .await
        .expect("recall runs");
    assert!(
        default_path
            .candidates
            .iter()
            .any(|candidate| candidate.summary.chars().count() > 8),
        "the fixture must carry a summary the default budget does not cut"
    );

    let short = RecallKnobs {
        summary_chars: 8,
        ..RecallKnobs::default()
    };
    let moved = recall
        .recall_with(&scope, QUERY, None, &options, &short)
        .await
        .expect("recall runs");
    assert_eq!(
        fact_ids(&default_path),
        fact_ids(&moved),
        "the summary budget decides what a candidate says, not which candidates come back"
    );
    assert!(
        moved
            .candidates
            .iter()
            .all(|candidate| candidate.summary.chars().count() <= 8),
        "the moved budget must reach the materialization"
    );
}

/// Bug this turns red: the injection path changing behaviour on the case the
/// R6 gate already pins — the same criteria `tests/eval_gate.rs` asserts,
/// replayed through the default assembly.
#[tokio::test]
async fn default_assembly_reproduces_the_gate() {
    let case = cases::focus_switch_case(0);
    let result = run_case(&case, &workspace("gate"))
        .await
        .expect("case runs");
    assert!(
        result.passed(),
        "focus-switch failed through the default assembly: {result:#?}"
    );
    assert_eq!(result.quality.recall_at_k, 1.0);
    assert_eq!(result.quality.wrong_evidence_rate, 0.0);
}

/// Bug this turns red: an empty run producing an invalid artifact instead of
/// an empty one — "nothing was run" is a describable run.
#[test]
fn empty_case_set_is_a_valid_empty_scoreboard() {
    let registry = tune::default_registry();
    let json = tune::scoreboard_json(&[], &registry);
    assert_eq!(
        json,
        tune::scoreboard_json(&[], &registry),
        "the same run must serialize to the same bytes"
    );
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(value["schema"].as_u64(), Some(1));
    assert_eq!(value["knobs"].as_array().map(Vec::len), Some(12));
    assert_eq!(value["report"]["cases"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        value["report"]["quality"]["recall_at_k"].as_f64(),
        Some(0.0)
    );
    assert_eq!(value["report"]["cost"]["embed_calls"].as_u64(), Some(0));
}
