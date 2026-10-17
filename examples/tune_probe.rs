//! Smoke probe for the tuning surface (stage doc 08 S2).
//!
//! Runs the two self-built eval cases through the real runner and prints the
//! scoreboard summary, then asks the attribution for the knobs of an
//! artificial miss. Run by CI (`.github/workflows/ci.yml`, "Smoke (tune
//! probe)") — it exits non-zero when a case stops passing, when the
//! attribution stops naming the stage, or when a stage's knob set stops being
//! the one the registry states.

use mnemo2u::core::traits::{FactStore, LexicalStore};
use mnemo2u::core::types::{Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::diag::{attribute, Stage};
use mnemo2u::eval::{cases, open_stores, run_case};
use mnemo2u::query::facts::{MemoryRecall, RecallOptions};
use mnemo2u::tune;

#[tokio::main]
async fn main() -> Result<(), String> {
    let registry = tune::default_registry();
    println!(
        "tune probe: registry       -> {} knobs, ids in registry order",
        registry.knobs().len()
    );

    // 1. The cases the gate replays, summarized by the scoreboard.
    let mut results = Vec::new();
    for case in cases::all(0) {
        let workspace = std::env::temp_dir().join(format!(
            "mnemo2u-tune-probe-{}-{}",
            std::process::id(),
            case.set
        ));
        let _ = std::fs::remove_dir_all(&workspace);
        let result = run_case(&case, &workspace).await?;
        if !result.passed() {
            return Err(format!(
                "case {} failed: {:?}",
                case.case_id, result.failures
            ));
        }
        results.push(result);
    }
    let scoreboard = tune::scoreboard_json(&results, &registry);
    let produced: serde_json::Value =
        serde_json::from_str(&scoreboard).map_err(|error| error.to_string())?;
    let report = &produced["report"];
    println!(
        "tune probe: scoreboard     -> recall={} miss={} wrong={} embed_calls={} schema={}",
        report["quality"]["recall_at_k"],
        report["quality"]["miss_rate"],
        report["quality"]["wrong_evidence_rate"],
        report["cost"]["embed_calls"],
        produced["schema"]
    );
    if report["quality"]["recall_at_k"].as_f64() != Some(1.0) {
        return Err(format!(
            "the default assembly stopped recalling everything it used to: {}",
            report["quality"]["recall_at_k"]
        ));
    }

    // 2. An artificial miss: a captured marker cut by `top_k = 0`.
    let workspace =
        std::env::temp_dir().join(format!("mnemo2u-tune-probe-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    let stores = open_stores(&workspace).await?;
    let scope = ScopeKey {
        workspace_id: "probe-ws".to_string(),
        session_id: None,
        task_id: None,
    };
    let text = "the chrysalis ledger rotated on tuesday";
    stores
        .facts
        .put(&Fact {
            id: FactId("f-probe".to_string()),
            scope: scope.clone(),
            kind: "semantic".to_string(),
            text: text.to_string(),
            source_refs: vec![SourceRef {
                workspace_id: scope.workspace_id.clone(),
                session_id: "probe-s1".to_string(),
                branch_id: "main".to_string(),
                epoch: 1,
                event_id: "ev-probe".to_string(),
            }],
            trust_state: TrustState::Verified,
            valid_from: None,
            valid_to: None,
            created_at: 1,
            superseded_by: None,
            revision: 0,
        })
        .await
        .map_err(|error| error.to_string())?;
    stores
        .lexical
        .index(&scope, &FactId("f-probe".to_string()), text)
        .await
        .map_err(|error| error.to_string())?;

    let recall = MemoryRecall {
        facts: stores.facts.clone(),
        vector: None,
        lexical: Some(stores.lexical.clone()),
        edges: Some(stores.edges.clone()),
        judge: None,
    };
    let options = RecallOptions {
        top_k: 0,
        rrf_k: 60,
        per_arm_limit: 50,
        staleness: None,
    };
    let outcome = recall
        .recall(&scope, "chrysalis ledger", None, &options)
        .await
        .map_err(|error| error.to_string())?;
    let cut = attribute(
        &recall,
        &scope,
        "chrysalis ledger",
        None,
        &options,
        "chrysalis ledger rotated",
        &outcome,
    )
    .await
    .map_err(|error| error.to_string())?;
    let knobs: Vec<&str> = tune::knobs_for(&cut.stage)
        .iter()
        .map(|knob| knob.id)
        .collect();
    println!("tune probe: miss stage     -> {:?}", cut.stage);
    println!("tune probe: knobs for it   -> {knobs:?}");
    println!(
        "tune probe: policy stage   -> {:?}",
        tune::knobs_for(&Stage::NotCaptured)
            .iter()
            .map(|knob| knob.id)
            .collect::<Vec<&str>>()
    );

    if cut.stage
        != (Stage::RankedOut {
            position: 0,
            top_k: 0,
        })
    {
        return Err(format!("captured-marker terminal drifted: {:?}", cut.stage));
    }
    let expected = [
        "recall.graph_hops",
        "recall.jev_candidate_cap",
        "recall.per_arm_limit",
        "recall.rel_correction",
        "recall.rel_support",
        "recall.rrf_k",
        "recall.seeds_per_arm",
        "recall.top_k",
    ];
    if knobs != expected {
        return Err(format!(
            "the knob set for a ranked-out miss drifted: {knobs:?}, expected {expected:?}"
        ));
    }
    if !tune::knobs_for(&Stage::NotCaptured).is_empty() {
        return Err("a policy stage grew knobs: it must stay empty".to_string());
    }
    if registry.get("recall.summary_chars").is_err() {
        return Err("the registry lost a knob".to_string());
    }
    Ok(())
}
