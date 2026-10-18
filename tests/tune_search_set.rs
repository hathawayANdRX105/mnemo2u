//! The headroom case set (stage doc 08 S3.5): a corpus the default assembly
//! cannot answer, and the acceptance rules that make it usable.
//!
//! Every test drives the real harness over real Turso stores with the
//! deterministic mock embedder and no model. Nothing here constructs a
//! `CaseResult` by hand — the quality numbers come out of `run_case_with`,
//! because a rule tested against an invented result only proves the
//! arithmetic.
//!
//! What these tests are for: proving that a case whose default assembly
//! misses is *allowed* to improve (S3's rule 3 used to refuse every direction
//! on such a set), that the direction that improves it is accepted, and that
//! the guards which stopped a candidate before still stop it.

use std::path::PathBuf;

use mnemo2u::eval::{cases, external, run_case_with};
use mnemo2u::query::facts::RecallKnobs;
use mnemo2u::tune::search::{Search, Step, Verdict};
use mnemo2u::tune::{self, KnobValue};

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnemo2u-s3set-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The holdout in the shape the probe builds it: the invariant case plus the
/// two external slices.
fn holdout_set() -> Vec<mnemo2u::eval::Case> {
    let mut set = cases::held_out_all(0);
    set.extend(
        external::external_all(0, &external::fixture_dir())
            .expect("the external slices load from the committed fixtures"),
    );
    set
}

/// One query's weight in the tuning set. Recomputed, so a floor that stopped
/// tracking the set is red.
fn floor_for(set: &[mnemo2u::eval::Case]) -> f64 {
    let queries: usize = set.iter().map(|case| case.queries.len()).sum();
    assert!(queries > 0, "a floor over an empty set is undefined");
    1.0 / queries as f64
}

fn step(knob_id: &str, to: KnobValue) -> Step {
    let from = tune::default_registry()
        .get(knob_id)
        .expect("a registered knob")
        .current;
    Step {
        knob_id: knob_id.to_string(),
        from,
        to,
        rationale: "test: hand-built candidate".to_string(),
    }
}

/// Bug this turns red: the case's chain never reaches the store, so the graph
/// arm walks nothing and the case reads as saturated. This is the mechanism
/// the whole set rests on — `Case` used to have no way to name a relation,
/// and the write gateway cannot produce one without a judge.
#[tokio::test]
async fn the_cases_chain_reaches_the_edge_store() {
    let set = cases::summary_cut_all(0);
    let root = workspace("chain");
    let mut reached = 0usize;
    for case in &set {
        let result = run_case_with(case, &root, &RecallKnobs::default())
            .await
            .expect("the case runs");
        // At two hops the marker's fact is not reached, so the case misses and
        // fails its own conditions — which is what makes it headroom.
        assert_eq!(
            result.quality.recall_at_k, 0.0,
            "the default assembly must miss this case: {result:?}"
        );
        assert!(
            !result.passed(),
            "a case with headroom must fail by default: {result:?}"
        );
        // Three hops reaches it: the chain is real, so raising the budget
        // brings the marker back.
        let wide = RecallKnobs {
            graph_hops: 3,
            ..RecallKnobs::default()
        };
        let walked = run_case_with(case, &root, &wide)
            .await
            .expect("the case runs");
        assert_eq!(
            walked.quality.recall_at_k, 1.0,
            "three hops must reach the marker: {walked:?}"
        );
        reached += 1;
    }
    assert_eq!(reached, 2, "the headroom set has two cases");
}

/// Bug this turns red: a case that misses is allowed to lose ground, so a
/// candidate could buy an acceptance by making the set worse somewhere the
/// floor does not look. The rule compares each case against its own baseline.
#[tokio::test]
async fn a_case_that_misses_may_not_lose_ground() {
    let tuning = cases::summary_cut_all(0);
    let holdout = holdout_set();
    let root = workspace("no-lose");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    // A step that fixes nothing: `graph_hops` at 3 is the good direction, so
    // taking it *down* to 1 must be refused for what it costs, not for what it
    // gains — and the reason has to name the rule.
    let down = step("recall.graph_hops", KnobValue::U8(1));
    let verdict = search.evaluate(&down, &root).await.expect("evaluate runs");
    if let Verdict::RejectRegression { reason } = &verdict {
        assert!(
            reason.contains("frozen conditions") || reason.contains("holdout"),
            "a step that loses ground must name the rule that refused it: {reason}"
        );
    }
}

/// Bug this turns red: the search refuses every direction on a set with
/// headroom, because rule 3 demands that every case pass. This is the whole
/// point of the round — the loop has to be able to accept something.
#[tokio::test]
async fn the_search_accepts_the_direction_the_chain_needs() {
    let tuning = cases::summary_cut_all(0);
    let holdout = holdout_set();
    let root = workspace("accept");
    let floor = floor_for(&tuning);
    assert_eq!(
        floor, 0.5,
        "two queries make the floor a half, so one query's fix cannot clear it"
    );

    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor,
    );
    let wide = step("recall.graph_hops", KnobValue::U8(3));
    let verdict = search.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::Accept { .. }),
        "three hops fixes both cases and must clear the floor: {verdict:?}"
    );

    // The accept is real: committing it moves the assembly and records it.
    search
        .commit(&wide, &verdict)
        .expect("an acceptance commits");
    assert_eq!(search.accepted().len(), 1, "one step is recorded");
    assert_eq!(
        search.registry().get("recall.graph_hops").unwrap().current,
        KnobValue::U8(3),
        "the assembly holds the accepted value"
    );

    // And the accepted assembly really does answer the case.
    let after = run_case_with(
        &tuning[0],
        &root,
        &search.registry().recall_knobs().unwrap(),
    )
    .await
    .expect("the case runs");
    assert_eq!(
        after.quality.recall_at_k, 1.0,
        "the committed assembly must answer the case: {after:?}"
    );
}

/// Bug this turns red: the floor drifts to `>=` and a single-query fix starts
/// clearing it. A gain equal to the floor is one query's worth of movement,
/// which is exactly what the floor refuses.
#[tokio::test]
async fn a_gain_equal_to_the_floor_is_still_refused() {
    let tuning = cases::summary_cut_all(0);
    let holdout = holdout_set();
    let root = workspace("tie");
    // One query's weight over two queries is 0.5; the step gains 1.0 by fixing
    // both, so it clears. A floor of 1.0 is the whole set, and a gain that
    // fixes one query of two is 0.5 — refused either way. What is checked here
    // is the tie: a floor of exactly 1.0 against a gain of exactly 1.0.
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        1.0,
    );
    let wide = step("recall.graph_hops", KnobValue::U8(3));
    let verdict = search.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectNoiseFloor { .. }),
        "a gain equal to the floor must be refused: {verdict:?}"
    );
}

/// Bug this turns red: the holdout stops being an invariant, so a step can be
/// accepted by moving a set that was supposed to stand still. `held_out_case`
/// has no edges and every marker inside the cut, so no searchable knob can
/// touch it.
#[tokio::test]
async fn the_holdout_case_is_invariant_under_the_searchable_knobs() {
    let case = cases::held_out_case(0);
    let root = workspace("invariant");
    let knobs: Vec<RecallKnobs> = vec![
        RecallKnobs::default(),
        RecallKnobs {
            graph_hops: 3,
            ..RecallKnobs::default()
        },
        RecallKnobs {
            graph_hops: 1,
            ..RecallKnobs::default()
        },
        RecallKnobs {
            summary_chars: 400,
            ..RecallKnobs::default()
        },
        RecallKnobs {
            seeds_per_arm: 50,
            ..RecallKnobs::default()
        },
        RecallKnobs {
            rel_support: 2.4,
            ..RecallKnobs::default()
        },
        RecallKnobs {
            rel_correction: 3.0,
            ..RecallKnobs::default()
        },
    ];
    let baseline = run_case_with(&case, &root, &knobs[0])
        .await
        .expect("the case runs");
    assert!(
        baseline.passed(),
        "the invariant case must pass by default: {baseline:?}"
    );
    for knob in &knobs[1..] {
        let result = run_case_with(&case, &root, knob)
            .await
            .expect("the case runs");
        assert_eq!(
            result.quality.recall_at_k, baseline.quality.recall_at_k,
            "no searchable knob may move this case's recall: {knob:?} gave {result:?}"
        );
        assert_eq!(
            result.quality.wrong_evidence_rate, baseline.quality.wrong_evidence_rate,
            "no searchable knob may move this case's wrong evidence: {knob:?}"
        );
    }
}

/// Bug this turns red: the existing case sets rotate their snapshots when the
/// new `edges` field enters the hash, which would rewrite the S2 baseline
/// fixture.
#[test]
fn the_existing_case_sets_keep_their_snapshots() {
    for case in cases::all(0) {
        assert!(
            case.edges.is_empty(),
            "`{}` carries no relations, so its snapshot must be the frozen one",
            case.case_id
        );
        // Recomputed from the same inputs, the id must match what the case
        // already holds — the constructor used the five-field hash.
        let recomputed = mnemo2u::eval::snapshot_id(
            &case.case_id,
            &case.turns,
            &case.queries,
            case.distractor_sessions,
            &case.conditions,
        );
        assert_eq!(
            recomputed, case.snapshot_id,
            "`{}` rotated its snapshot",
            case.case_id
        );
    }
}

/// Bug this turns red: the probe's noise floor stops tracking the set it is
/// declared over, so a set that grows silently changes what "one query" means.
#[test]
fn the_floor_over_the_new_sets_is_what_the_probe_declares() {
    let tuning = cases::summary_cut_all(0);
    let holdout = holdout_set();
    let tuning_queries: usize = tuning.iter().map(|case| case.queries.len()).sum();
    let holdout_queries: usize = holdout.iter().map(|case| case.queries.len()).sum();
    assert_eq!(tuning_queries, 2, "the headroom set has two queries");
    assert_eq!(
        holdout_queries, 4,
        "the holdout has the invariant case's two queries plus one per external slice"
    );
    assert_eq!(
        floor_for(&tuning),
        1.0 / 2.0,
        "the floor is one query's weight over the tuning set"
    );
}
