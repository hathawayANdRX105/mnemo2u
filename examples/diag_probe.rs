//! Smoke probe for the read-path diagnosis (stage doc 07 S1).
//!
//! Opens the durable Turso stores the memory layer runs on, writes one
//! captured row, and asserts the two attribution terminals a CI run cares
//! about: an existing row cut by `top_k = 0` is `RankedOut`, and a marker
//! nobody wrote is `NotCaptured`. Run by CI (`.github/workflows/ci.yml`,
//! "Smoke (diag probe)" step) — the probe exits non-zero when a terminal
//! stops holding.

use mnemo2u::core::traits::{FactStore, LexicalStore};
use mnemo2u::core::types::{Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::diag::{attribute, health, Stage};
use mnemo2u::eval::open_stores;
use mnemo2u::query::facts::{MemoryRecall, RecallOptions};

#[tokio::main]
async fn main() -> Result<(), String> {
    let workspace = std::env::temp_dir().join(format!("mnemo2u-diag-probe-{}", std::process::id()));
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
    // `top_k = 0` keeps the run honest: the row exists and is retrievable,
    // so the verdict must be the cut, not a capture or arm failure.
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
    let missing = attribute(
        &recall,
        &scope,
        "chrysalis ledger",
        None,
        &options,
        "never written marker",
        &outcome,
    )
    .await
    .map_err(|error| error.to_string())?;

    println!("diag probe: health row      -> {:?}", health(&outcome, 0));
    println!("diag probe: captured marker -> {:?}", cut.stage);
    println!("diag probe: absent marker   -> {:?}", missing.stage);

    if cut.stage
        != (Stage::RankedOut {
            position: 0,
            top_k: 0,
        })
    {
        return Err(format!("captured-marker terminal drifted: {:?}", cut.stage));
    }
    if missing.stage != Stage::NotCaptured {
        return Err(format!(
            "absent-marker terminal drifted: {:?}",
            missing.stage
        ));
    }
    Ok(())
}
