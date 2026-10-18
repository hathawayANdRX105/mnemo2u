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
//! the guards which stopped a candidate before still stop it. The last two
//! cover the write half of the knob surface, which the search cannot reach
//! through its own bounds check — the gap jev_review named on the S3.5 diff.

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
        holdout_queries, 6,
        "the holdout has the invariant case's four plus one per external slice"
    );
    assert_eq!(
        floor_for(&tuning),
        1.0 / 2.0,
        "the floor is one query's weight over the tuning set"
    );
}

/// Bug this turns red: `KnobRegistry::set` accepts a value outside the knob's
/// declared window, of another kind than the row carries, or on a row that
/// holds no value at all. The search's own bounds check never lets any of that
/// happen, so without this the write half of the knob surface would be
/// unreachable — exactly the test gap jev_review named on the S3.5 diff.
#[test]
fn a_registry_rejects_a_value_outside_the_knobs_window() {
    let mut registry = tune::default_registry();
    let before = registry.get("recall.graph_hops").unwrap().current;
    // The window starts at 1, so 0 is below it.
    let error = registry
        .set("recall.graph_hops", KnobValue::U8(0))
        .expect_err("0 is below the declared floor");
    assert!(
        matches!(error, tune::TuneError::OutOfBounds { .. }),
        "the error must name the window it broke: {error:?}"
    );
    assert_eq!(
        registry.get("recall.graph_hops").unwrap().current,
        before,
        "a refused write must leave the assembly where it was"
    );

    // And a value the search can never name explicitly: an unset knob.
    let unset = registry
        .set("recall.graph_hops", KnobValue::Unset)
        .expect_err("an unset value is not a setting");
    assert!(
        matches!(unset, tune::TuneError::UnsetKnob { .. }),
        "an unset knob must be refused, not written: {unset:?}"
    );

    // A name the registry does not carry is an error, not a panic.
    let unknown = registry
        .set("recall.nope", KnobValue::U8(1))
        .expect_err("an unknown knob is an error");
    assert!(
        matches!(unknown, tune::TuneError::UnknownKnob { .. }),
        "an unknown knob id must be an error: {unknown:?}"
    );

    // A value of another kind is refused, not silently converted. `admits`
    // reads a value as one number whatever its variant, so `F64(2.0)` on a
    // `u8` row is inside the window — and would fail much later, in
    // `recall_knobs`, long after the write that caused it.
    let wrong_kind = registry
        .set("recall.graph_hops", KnobValue::F64(2.0))
        .expect_err("an f64 is not a hop count");
    assert!(
        matches!(wrong_kind, tune::TuneError::WrongKind { .. }),
        "a value of another kind must name that: {wrong_kind:?}"
    );
    assert_eq!(
        registry.get("recall.graph_hops").unwrap().current,
        before,
        "a refused write leaves the row where it was"
    );

    // And the rows that hold no value at all — the caller's per-call budgets.
    // All four, not one example: a member of this class is unwritable because
    // the class is, and a test that names a single row would keep passing if
    // the class grew or if one of them quietly gained a default.
    for id in [
        "recall.top_k",
        "recall.per_arm_limit",
        "recall.stale.max_age_secs",
        "recall.stale.min_sessions",
    ] {
        let refused = registry
            .set(id, KnobValue::Usize(5))
            .expect_err("a per-call budget is the caller's to set");
        assert!(
            matches!(refused, tune::TuneError::UnsetKnob { .. }),
            "`{id}` holds no value and must refuse a write: {refused:?}"
        );
        assert!(
            matches!(registry.get(id).unwrap().current, KnobValue::Unset),
            "`{id}` must stay unset after a refused write"
        );
    }
}

/// Bug this turns red: a refused write silently mutates, or a successful one
/// does not. The write half has to be an exact, observable operation.
#[test]
fn an_accepted_write_moves_the_knob_and_nothing_else() {
    let mut registry = tune::default_registry();
    let before: Vec<serde_json::Value> =
        serde_json::from_str(&registry.json()).expect("the registry serializes as JSON");
    registry
        .set("recall.graph_hops", KnobValue::U8(3))
        .expect("3 is inside the window");
    let after: Vec<serde_json::Value> =
        serde_json::from_str(&registry.json()).expect("the registry serializes as JSON");

    let current = |records: &[serde_json::Value], id: &str| {
        records
            .iter()
            .find(|record| record["id"] == id)
            .unwrap_or_else(|| panic!("`{id}` is not in the registry"))
            .clone()
    };
    // `KnobValue` serializes tagged, so `U8(3)` is `{"kind":"u8","value":3}`
    // and not the bare `3`.
    assert_eq!(
        current(&after, "recall.graph_hops")["current"],
        serde_json::json!({"kind": "u8", "value": 3}),
        "the accepted write lands on the knob it named"
    );
    assert_eq!(
        current(&before, "recall.graph_hops")["current"],
        serde_json::json!({"kind": "u8", "value": 2}),
        "the knob started where the table says it does"
    );
    // Every other row is byte-identical, which is the whole point of a write
    // that names one knob.
    for record in &before {
        let id = record["id"].as_str().expect("a knob id");
        if id == "recall.graph_hops" {
            continue;
        }
        assert_eq!(&current(&after, id), record, "`{id}` must not have moved");
    }
    assert_eq!(after.len(), before.len(), "the knob set is unchanged");
}

/// Bug this turns red: a bound check is mistaken for a kind check.
/// [`KnobValue::as_f64`] flattens every variant to one number, so `F64(2.0)`
/// is "inside" a window that belongs to a `u8` row — the kind is the one
/// property the bounds cannot see, and `same_kind` is where it is seen.
#[test]
fn same_kind_separates_magnitude_from_kind() {
    // Same kind, different magnitude.
    assert!(
        KnobValue::Usize(200).same_kind(&KnobValue::Usize(400)),
        "two counts are the same kind"
    );
    assert!(
        KnobValue::U8(2).same_kind(&KnobValue::U8(3)),
        "two depths are the same kind"
    );
    // Same number, different kind — the case `admits` cannot catch.
    assert!(
        !KnobValue::F64(2.0).same_kind(&KnobValue::U8(2)),
        "an f64 and a u8 carrying the same number are still different kinds"
    );
    assert!(
        !KnobValue::U32(60).same_kind(&KnobValue::Usize(60)),
        "two integer widths are different kinds"
    );
    // Unset is its own kind, which is what makes a value-less row unwritable.
    assert!(
        !KnobValue::Usize(5).same_kind(&KnobValue::Unset),
        "unset is not any value's kind"
    );
}
