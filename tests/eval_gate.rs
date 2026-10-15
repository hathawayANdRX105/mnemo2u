//! The evaluation gate (stage doc 06 T6): frozen cases run in CI, a
//! regression on any axis is red.
//!
//! The thresholds asserted here are the frozen criteria — the stage doc's
//! ordering is freeze first, score second, and the snapshot id in each case
//! is what makes that ordering auditable: editing a corpus or a query
//! changes the snapshot, so a silent criteria change has to be committed.
//!
//! Scale axis (stage doc 06 AC3): the same cases run at 1, 10 and 50
//! distractor sessions. Quality must not degrade with scale, and the
//! latency ceiling is a regression guard rather than a performance claim —
//! the numbers are printed so a change is visible in the log.

use std::path::PathBuf;

use mnemo2u::eval::cases;
use mnemo2u::eval::{run_case, Case, Report};

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnemo2u-eval-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Run one case in a fresh workspace and report it.
async fn run(case: &Case, name: &str) -> mnemo2u::eval::CaseResult {
    run_case(case, &workspace(name)).await.expect("case runs")
}

#[tokio::test]
async fn focus_switch_recalls_early_evidence_after_a_topic_switch() {
    let case = cases::focus_switch_case(0);
    let result = run(&case, "focus-switch").await;
    assert!(
        result.passed(),
        "focus-switch failed: {result:#?}
manifest: {}",
        mnemo2u::eval::Report::manifest_json(&case, &result).expect("manifest")
    );
    assert_eq!(result.quality.recall_at_k, 1.0);
    assert_eq!(result.quality.wrong_evidence_rate, 0.0);
}

#[tokio::test]
async fn exact_identifiers_are_recalled_over_distractors() {
    let case = cases::identifier_case(0);
    let result = run(&case, "identifier").await;
    assert!(result.passed(), "identifier failed: {result:#?}");
    assert_eq!(result.quality.recall_at_k, 1.0);
    // Five queries, each answering with its own identifier: the forbidden
    // markers must never surface, and the right identifier must come back
    // first rather than merely somewhere in the top-k.
    assert_eq!(result.queries.len(), 5);
    assert!(
        result
            .queries
            .iter()
            .all(|query| query.wrong_evidence.is_empty()),
        "wrong evidence: {:#?}",
        result.queries
    );
    assert!(
        result.queries.iter().all(|query| query.top_hit),
        "top-ranked candidate missed the expected marker: {:#?}",
        result.queries
    );
}

/// The scale axis: the same cases at 1, 10 and 50 distractor sessions.
/// Quality must hold; latency is reported, and the ceiling is a guard
/// against a pathological regression rather than a performance budget.
#[tokio::test]
async fn quality_holds_across_scale() {
    let mut all = Vec::new();
    for scale in [1usize, 10, 50] {
        for case in cases::all(scale) {
            let result = run(&case, &format!("scale-{}-{}", scale, case.set)).await;
            println!(
                "scale={scale} set={} recall={:.2} miss={:.2} wrong={:.2} p50={}µs p95={}µs",
                case.set,
                result.quality.recall_at_k,
                result.quality.miss_rate,
                result.quality.wrong_evidence_rate,
                result.latency.p50_us,
                result.latency.p95_us,
            );
            assert!(
                result.quality.recall_at_k >= case.conditions.min_recall,
                "{} at scale {scale} regressed: {result:#?}",
                case.case_id
            );
            assert!(
                result.quality.wrong_evidence_rate == 0.0,
                "{} at scale {scale} surfaced wrong evidence: {result:#?}",
                case.case_id
            );
            all.push(result);
        }
    }

    let report = Report::from_cases(all);
    println!(
        "aggregate: recall={:.2} miss={:.2} wrong={:.2} worst_p50={}µs worst_p95={}µs",
        report.quality.recall_at_k,
        report.quality.miss_rate,
        report.quality.wrong_evidence_rate,
        report.latency.p50_us,
        report.latency.p95_us,
    );
    assert!(report.passed(), "aggregate gate failed: {report:#?}");

    // The latency guard: a local recall over a few hundred rows should stay
    // far below the 500 ms budget the AC4 baseline set. This is a
    // regression tripwire, not a latency claim.
    assert!(
        report.latency.p95_us < 500_000,
        "worst-case P95 = {}µs exceeds the 500ms guard",
        report.latency.p95_us
    );
}

/// The frozen criteria are auditable: every case's snapshot id is stable
/// across runs, and a corpus edit changes it (which is what forces a
/// criteria change to be committed).
#[tokio::test]
async fn snapshots_are_stable_and_corpus_edits_change_them() {
    let first = cases::focus_switch_case(0);
    let again = cases::focus_switch_case(0);
    assert_eq!(
        first.snapshot_id, again.snapshot_id,
        "the same corpus must produce the same snapshot"
    );

    let scale_one = cases::focus_switch_case(1);
    assert_ne!(
        first.snapshot_id, scale_one.snapshot_id,
        "a different scale is a different corpus"
    );

    // A corpus edit: one turn's text changes, so the effective corpus does.
    // `snapshot_id` is computed when a case is *built*, so the edited case is
    // rebuilt the way a case-set source edit would be — that is the path the
    // freeze ordering audits.
    let mut edited = first.clone();
    edited.turns[0].content = "a different opening turn".into();
    edited.snapshot_id = mnemo2u::eval::snapshot_id(
        &edited.case_id,
        &edited.turns,
        &edited.queries,
        edited.distractor_sessions,
        &edited.conditions,
    );
    assert_ne!(
        first.snapshot_id, edited.snapshot_id,
        "editing a turn must rotate the snapshot"
    );

    // A query edit.
    let mut edited = first.clone();
    edited.queries[0].text = "a different query".into();
    edited.snapshot_id = mnemo2u::eval::snapshot_id(
        &edited.case_id,
        &edited.turns,
        &edited.queries,
        edited.distractor_sessions,
        &edited.conditions,
    );
    assert_ne!(
        first.snapshot_id, edited.snapshot_id,
        "editing a query must rotate the snapshot"
    );

    // A threshold edit. Without the conditions in the hash payload this is
    // the silent criteria change the freeze ordering exists to catch:
    // loosening `min_recall` would leave the snapshot byte-identical while
    // the gate read the new, weaker bar.
    let mut loosened = first.clone();
    loosened.conditions.min_recall = 0.5;
    loosened.snapshot_id = mnemo2u::eval::snapshot_id(
        &loosened.case_id,
        &loosened.turns,
        &loosened.queries,
        loosened.distractor_sessions,
        &loosened.conditions,
    );
    assert_ne!(
        first.snapshot_id, loosened.snapshot_id,
        "loosening a frozen threshold must rotate the snapshot"
    );
}
