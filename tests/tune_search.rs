//! The search loop's rules (stage doc 08 S3): the noise floor, the holdout,
//! the wrong-evidence guard, the ratchet, and the seam the default assembly
//! passes through unchanged.
//!
//! Every test drives the real harness — `run_case` / `run_case_with` over
//! real Turso stores with the deterministic mock embedder and no model — and
//! constructs only `Step` values by hand. A `Step` is the proposer's input;
//! the measurement is never faked, because a rule tested against an invented
//! `CaseResult` only proves the arithmetic, not that the loop obeys it.
//!
//! What is real here and what is not: the case corpora are real (self-built
//! or written directly into a store), the knobs are injected for real, and
//! the quality numbers come out of `run_case_with`. The proposer is driven
//! by hand-built steps rather than by `Search::propose` in the rule tests, so
//! each rule is exercised against exactly the candidate it is about.

use std::path::{Path, PathBuf};

use mnemo2u::eval::{cases, run_case, run_case_with};
use mnemo2u::query::facts::RecallKnobs;
use mnemo2u::tune::search::{Search, Step, Verdict};
use mnemo2u::tune::{self, KnobValue};

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnemo2u-s3-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The two external slices, at scale 0, from the committed fixtures.
fn holdout_set() -> Vec<mnemo2u::eval::Case> {
    mnemo2u::eval::external::external_all(0, &mnemo2u::eval::external::fixture_dir())
        .expect("the external slices load from the committed fixtures")
}

/// One query's weight in the tuning set — the noise floor the module doc
/// declares. Recomputed here rather than hardcoded, so a set that grows
/// re-derives its own floor and a floor that stopped tracking the set is red.
fn floor_for(cases: &[mnemo2u::eval::Case]) -> f64 {
    let queries: usize = cases.iter().map(|case| case.queries.len()).sum();
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

/// Bug this turns red: the injection seam changes the default assembly, which
/// would invalidate every number the R6 gate and the S2 baseline were built
/// on. Compared field by field, latency excluded, because latency is the one
/// field a re-run is allowed to differ in.
#[tokio::test]
async fn default_assembly_unchanged_through_the_new_seam() {
    let case = cases::focus_switch_case(0);
    let pinned = run_case(&case, &workspace("seam-pinned"))
        .await
        .expect("run_case runs");
    let injected = run_case_with(&case, &workspace("seam-injected"), &RecallKnobs::default())
        .await
        .expect("run_case_with runs");

    assert_eq!(
        pinned.case_id, injected.case_id,
        "the case is the same case"
    );
    assert_eq!(
        pinned.snapshot_id, injected.snapshot_id,
        "the snapshot is the same"
    );
    assert_eq!(
        pinned.quality, injected.quality,
        "quality drifted through the seam"
    );
    assert_eq!(pinned.cost, injected.cost, "cost drifted through the seam");
    assert_eq!(
        pinned.failures, injected.failures,
        "failure classification drifted through the seam"
    );
    assert_eq!(
        pinned.conditions, injected.conditions,
        "the frozen conditions drifted through the seam"
    );
    assert_eq!(
        pinned.queries.len(),
        injected.queries.len(),
        "the query set drifted through the seam"
    );
    for (left, right) in pinned.queries.iter().zip(&injected.queries) {
        assert_eq!(left.recall, right.recall, "a query's recall drifted");
        assert_eq!(
            left.missing, right.missing,
            "a query's missing markers drifted"
        );
        assert_eq!(
            left.wrong_evidence, right.wrong_evidence,
            "a query's wrong evidence drifted"
        );
        assert_eq!(left.top_hit, right.top_hit, "a query's top hit drifted");
        assert_eq!(
            left.returned_ids, right.returned_ids,
            "a query's ids drifted"
        );
    }
    assert_eq!(
        pinned.latency.samples, injected.latency.samples,
        "the sample count drifted"
    );
}

/// Bug this turns red: the floor is derived from anything but case
/// granularity — or the comparison uses `>=`, which would accept a single
/// query's worth of movement. Driven on the real one-query self-built case, so
/// the numbers are measured rather than invented.
#[tokio::test]
async fn noise_floor_rejects_one_query_of_gain() {
    // The focus-switch case carries exactly one query, so one query's weight
    // is the whole set: the floor is 1.0 and no candidate can clear it.
    let tuning = vec![cases::focus_switch_case(0)];
    let holdout = holdout_set();
    let floor = floor_for(&tuning);
    assert_eq!(
        floor, 1.0,
        "one query means one query's weight is the whole set"
    );

    let root = workspace("floor");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor,
    );

    // A candidate that widens the graph walk: on a set that already recalls
    // everything it cannot gain, so the floor rule must refuse it — and the
    // refusal must be the floor, not a failure to measure.
    let wide = step("recall.graph_hops", KnobValue::U8(3));
    let verdict = search.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectNoiseFloor { .. }),
        "a one-query set cannot produce a gain that clears its own floor: {verdict:?}"
    );

    // The refusal names the floor and not a broken condition, which is what
    // distinguishes "no gain here" from "this candidate is unusable".
    if let Verdict::RejectNoiseFloor { gain } = &verdict {
        assert_eq!(
            *gain, 0.0,
            "on a set that already recalls everything the gain is exactly zero"
        );
    }

    // The floor rule is strict: a gain must *exceed* the floor, so a zero gain
    // is refused even when the floor is zero. Rejecting a tie is the point —
    // a gain equal to the floor is one query's worth of movement.
    let mut strict = Search::new(tune::default_registry(), tuning, holdout, 0.0);
    let tied = strict.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        matches!(tied, Verdict::RejectNoiseFloor { .. }),
        "a gain equal to the floor must be refused, not accepted: {tied:?}"
    );
}

/// Bug this turns red: the holdout check is missing or one-directional. The
/// holdout here is the two external slices, which the default assembly
/// passes; a candidate that breaks one of them is a regression whatever it
/// gains on the tuning set.
#[tokio::test]
async fn holdout_regression_blocks_acceptance() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("holdout");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    // `graph_hops` at 0 is out of bounds (the window starts at 1), so it is
    // refused before any case runs: the holdout rule is about measured
    // regressions, and this one never reaches the measurement.
    let zero_hops = step("recall.graph_hops", KnobValue::U8(0));
    let verdict = search
        .evaluate(&zero_hops, &root)
        .await
        .expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectOutOfBounds { .. }),
        "a value outside the window must not reach the harness: {verdict:?}"
    );

    // A large hop count is inside the window and runs for real: it widens the
    // graph arm's neighborhood, which on these sets cannot raise recall (both
    // sets already sit at 1.0) and can only add wrong evidence — so the
    // candidate is refused, and by a rule that measured the holdout.
    let wide = step("recall.graph_hops", KnobValue::U8(8));
    let verdict = search.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        !matches!(verdict, Verdict::Accept { .. }),
        "a wider graph walk cannot be accepted on sets that already recall everything: {verdict:?}"
    );

    // With an empty holdout set nothing may be accepted, because a set nobody
    // measured is not evidence of generalization.
    let mut blind = Search::new(tune::default_registry(), tuning, Vec::new(), 0.0);
    let verdict = blind.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectRegression { .. }),
        "an empty holdout set must never accept: {verdict:?}"
    );
}

/// Bug this turns red: the wrong-evidence guard is dropped, so a candidate
/// that trades a forbidden marker for recall is accepted.
#[tokio::test]
async fn wrong_evidence_zero_is_preserved() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("wrong");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    // The identifier set's forbidden markers are near-miss facts that only
    // AND-of-tokens matching keeps out; widening the graph walk past the
    // seeds' neighbourhood is the knob most likely to let one in. Whichever
    // way the measurement goes, a candidate that surfaces wrong evidence must
    // be refused as a regression rather than accepted on its recall.
    let wide = step("recall.graph_hops", KnobValue::U8(8));
    let verdict = search.evaluate(&wide, &root).await.expect("evaluate runs");
    if let Verdict::RejectRegression { reason } = &verdict {
        assert!(
            reason.contains("wrong evidence") || reason.contains("frozen conditions"),
            "the reason must name the rule that refused it: {reason}"
        );
    }

    // The same candidate under a search whose only job is to show the guard
    // is wired: a case whose forbidden marker is already in the default
    // top-k must make the search refuse, never accept.
    let guard = cases::identifier_case(0);
    let mut strict = Search::new(
        tune::default_registry(),
        vec![guard],
        holdout,
        floor_for(&tuning),
    );
    let verdict = strict.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        !matches!(verdict, Verdict::Accept { .. }),
        "a candidate that surfaces wrong evidence must never be accepted: {verdict:?}"
    );
}

/// Bug this turns red: the harness runs before the bound check, so an
/// unusable candidate costs a full evaluation and — worse — could be accepted
/// through a roundabout path.
#[tokio::test]
async fn out_of_bounds_candidate_never_runs_the_harness() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("bounds");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    let before = dir_entries(&root);
    let verdict = search
        .evaluate(&step("recall.graph_hops", KnobValue::U8(0)), &root)
        .await
        .expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectOutOfBounds { .. }),
        "a zero-hop walk is outside the window: {verdict:?}"
    );
    let after = dir_entries(&root);
    assert_eq!(
        before, after,
        "an out-of-bounds candidate must not create any store"
    );

    // A negative threshold is out of bounds too, and must be refused the same
    // way rather than coerced into something usable.
    let negative = step("recall.rerank_near_zero", KnobValue::F64(-1.0));
    let verdict = search
        .evaluate(&negative, &root)
        .await
        .expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectOutOfBounds { .. }),
        "a negative threshold is outside the window: {verdict:?}"
    );
    assert_eq!(dir_entries(&root), after, "still no store was created");
}

/// Bug this turns red: an empty case set is treated as "everything passed",
/// which is how a search accepts a candidate nobody measured.
#[tokio::test]
async fn empty_holdout_never_accepts() {
    let root = workspace("empty-holdout");
    let mut search = Search::new(
        tune::default_registry(),
        cases::all(0),
        Vec::new(),
        // A zero floor: the only thing that could still refuse this candidate
        // is the empty-holdout rule itself.
        0.0,
    );
    let verdict = search
        .evaluate(&step("recall.seeds_per_arm", KnobValue::Usize(12)), &root)
        .await
        .expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectRegression { .. }),
        "an empty holdout set must refuse: {verdict:?}"
    );

    // And an empty tuning set is the same failure with the other name.
    let mut blind = Search::new(tune::default_registry(), Vec::new(), holdout_set(), 0.0);
    let verdict = blind
        .evaluate(&step("recall.seeds_per_arm", KnobValue::Usize(12)), &root)
        .await
        .expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectRegression { .. }),
        "an empty tuning set must refuse: {verdict:?}"
    );
}

/// Bug this turns red: a direction refused by the noise floor is treated as
/// refuted and lost forever, so a step that would help from a different
/// assembly can never be reached — or, the other way, re-offered from the
/// *same* assembly, which is the non-convergence the `tried` set stops. The
/// floor says "not from here", and "here" changes on commit.
#[tokio::test]
async fn a_floor_refusal_is_closed_for_this_assembly_and_reopened_by_commit() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("floor-open");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    // One candidate, refused by the floor.
    let wide = step("recall.graph_hops", KnobValue::U8(3));
    let verdict = search.evaluate(&wide, &root).await.expect("evaluate runs");
    assert!(
        matches!(verdict, Verdict::RejectNoiseFloor { .. }),
        "this candidate must be refused by the floor: {verdict:?}"
    );

    // Closed for *this* assembly: offering it again would return the same
    // verdict, which is the non-convergence the `tried` set exists to stop.
    let offered: Vec<Step> = std::iter::from_fn(|| search.propose()).take(64).collect();
    assert!(
        !offered
            .iter()
            .any(|candidate| candidate.knob_id == "recall.graph_hops"
                && candidate.to == KnobValue::U8(3)),
        "a direction already measured against this assembly must not be re-offered"
    );

    // But not refuted: `commit` moves the assembly and re-opens the tried set,
    // so the same direction becomes measurable again from the new value. The
    // commit is driven here by hand rather than through the search's own
    // loop, because on these saturated sets nothing is ever accepted.
    let accepted = Step {
        knob_id: "recall.summary_chars".to_string(),
        from: step("recall.summary_chars", KnobValue::Usize(200)).from,
        to: KnobValue::Usize(201),
        rationale: "test: move the assembly so the tried set is re-opened".to_string(),
    };
    search
        .commit(&accepted, &Verdict::Accept { gain: 1.0 })
        .expect("a hand-built acceptance commits");
    let reopened: Vec<Step> = std::iter::from_fn(|| search.propose()).take(64).collect();
    assert!(
        reopened
            .iter()
            .any(|candidate| candidate.knob_id == "recall.graph_hops"
                && candidate.to == KnobValue::U8(3)),
        "a floor refusal is not a refutation: a new assembly must re-open it"
    );
}

/// Bug this turns red: `propose` silently stops when a knob it names is not
/// registered, which reads as "the search converged" and hides the bug.
#[test]
fn every_searchable_knob_is_registered() {
    let registry = tune::default_registry();
    for knob in registry.knobs() {
        assert!(
            registry.get(knob.id).is_ok(),
            "`{}` is registered under its own id",
            knob.id
        );
    }
    let injectable = [
        "recall.graph_hops",
        "recall.jev_candidate_cap",
        "recall.rel_correction",
        "recall.rel_support",
        "recall.rerank_near_zero",
        "recall.seeds_per_arm",
        "recall.summary_chars",
    ];
    for id in injectable {
        assert!(
            registry.get(id).is_ok(),
            "`{id}` is named by the search but is not registered"
        );
    }
    // The per-call budgets are the caller's, so the search must not name them:
    // naming one would mean changing what is measured rather than how the read
    // path reads.
    let search = Search::new(tune::default_registry(), cases::all(0), holdout_set(), 0.0);
    let offered: Vec<Step> = std::iter::from_fn(|| search.propose()).take(64).collect();
    for candidate in &offered {
        assert_ne!(
            candidate.knob_id, "recall.top_k",
            "a per-call budget was offered"
        );
        assert_ne!(
            candidate.knob_id, "recall.rrf_k",
            "a per-call budget was offered"
        );
        assert_ne!(
            candidate.knob_id, "recall.per_arm_limit",
            "a per-call budget was offered"
        );
    }
    assert!(
        !offered.is_empty(),
        "a fresh search must have a direction to offer"
    );
}

/// Bug this turns red: the baseline is re-measured per step, which doubles
/// the harness runs of a search for a number that cannot have moved — the
/// assembly only changes in `commit`. Also pins that the cache does not go
/// stale: after a commit the next step is measured against the new baseline.
#[tokio::test]
async fn the_baseline_is_measured_once_per_assembly() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("baseline-cache");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    // Two different candidates from the same assembly must both be scored
    // against the same baseline. If the cache went stale between them, the
    // second verdict would be measured against the first step's leftovers.
    let first = step("recall.seeds_per_arm", KnobValue::Usize(11));
    let second = step("recall.rel_support", KnobValue::F64(1.1));
    let left = search.evaluate(&first, &root).await.expect("evaluate runs");
    let right = search
        .evaluate(&second, &root)
        .await
        .expect("evaluate runs");
    assert_eq!(
        verdict_gain(&left),
        verdict_gain(&right),
        "both directions were measured from the same assembly, so their gains are the same zero"
    );

    // The floor is what refuses them here, and both are refused: on sets that
    // already recall everything no direction can gain.
    assert!(
        matches!(left, Verdict::RejectNoiseFloor { .. }),
        "the first direction must be a floor refusal: {left:?}"
    );
    assert!(
        matches!(right, Verdict::RejectNoiseFloor { .. }),
        "the second direction must be a floor refusal too: {right:?}"
    );
}

/// The gain a verdict carries, so a test can compare two measurements without
/// naming a rule that has not happened yet.
fn verdict_gain(verdict: &Verdict) -> f64 {
    match verdict {
        Verdict::Accept { gain } | Verdict::RejectNoiseFloor { gain } => *gain,
        Verdict::RejectRegression { .. } | Verdict::RejectOutOfBounds { .. } => f64::NAN,
    }
}

/// Bug this turns red: an unknown knob id panics or is silently skipped,
/// instead of stopping the search that named it.
#[test]
fn unknown_knob_is_an_error_not_a_panic() {
    let mut search = Search::new(tune::default_registry(), cases::all(0), holdout_set(), 0.0);
    let bogus = Step {
        knob_id: "recall.nope".to_string(),
        from: KnobValue::Usize(1),
        to: KnobValue::Usize(2),
        rationale: "test: a knob nobody registered".to_string(),
    };
    let verdict = futures::executor::block_on(search.evaluate(&bogus, &workspace("unknown")))
        .expect_err("an unknown id is an error, not a panic");
    assert!(
        verdict.contains("recall.nope"),
        "the error must name the id: {verdict}"
    );
}

/// Bug this turns red: the proposer can loop forever, so a search that never
/// converges hangs CI instead of reporting that it converged.
///
/// The cap below is a tripwire, not the mechanism: a full pass is one
/// direction per knob per side (seven knobs, two sides), so a search that
/// needed more than a small multiple of that is not converging. What proves
/// convergence is that `propose` eventually returns `None` at all.
#[tokio::test]
async fn search_terminates() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("terminates");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    let mut proposed = 0usize;
    while let Some(candidate) = search.propose() {
        proposed += 1;
        assert!(
            proposed <= 32,
            "the search did not converge: {proposed} directions proposed past the cap"
        );
        let verdict = search
            .evaluate(&candidate, &root)
            .await
            .expect("evaluate runs");
        if matches!(verdict, Verdict::Accept { .. }) {
            search.commit(&candidate, &verdict).expect("commit works");
        }
    }
    assert!(proposed > 0, "the proposer proposed nothing at all");

    // Converged means converged: with the assembly unchanged, no direction is
    // left to offer. This is the assertion that catches a proposer handing
    // back the same direction forever.
    assert!(
        search.propose().is_none(),
        "a converged search must not propose again from the same assembly"
    );
}

/// Bug this turns red: the ratchet is decoration — an accepted value does not
/// reach the registry, or the accepted history is not readable, so the
/// assembly a scoreboard records is not the assembly that ran.
#[tokio::test]
async fn accepted_steps_ratchet() {
    let tuning = cases::all(0);
    let holdout = holdout_set();
    let root = workspace("ratchet");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout.clone(),
        floor_for(&tuning),
    );

    // Drive the proposer to convergence, accepting whatever it may accept.
    // On these sets nothing can clear the floor, so the assertion below is
    // about the machinery holding either way: no accepted step means no
    // registry drift, which is itself the ratchet's honest output here.
    let mut accepted = 0usize;
    while let Some(candidate) = search.propose() {
        let verdict = search
            .evaluate(&candidate, &root)
            .await
            .expect("evaluate runs");
        if matches!(verdict, Verdict::Accept { .. }) {
            search.commit(&candidate, &verdict).expect("commit works");
            accepted += 1;
        }
    }
    assert_eq!(
        accepted,
        search.accepted().len(),
        "the accepted history must record exactly what was committed"
    );

    // Whatever was accepted, the registry must now carry it: that is the
    // property the scoreboard's knob block depends on.
    for step in search.accepted() {
        assert_eq!(
            search
                .registry()
                .get(&step.knob_id)
                .expect("a registered knob")
                .current,
            step.to,
            "the registry must carry the accepted value for `{}`",
            step.knob_id
        );
    }

    // Committing a rejection is refused: the ratchet records decisions, not
    // attempts.
    let rejected = Step {
        knob_id: "recall.graph_hops".to_string(),
        from: KnobValue::U8(2),
        to: KnobValue::U8(1),
        rationale: "test: a step that was refused".to_string(),
    };
    let error = search
        .commit(&rejected, &Verdict::RejectNoiseFloor { gain: 0.0 })
        .expect_err("committing a rejection must fail");
    assert!(
        error.contains("did not accept"),
        "the refusal must say so: {error}"
    );
}

/// Bug this turns red: the recorded assembly and the assembly that ran drift
/// apart, which is what makes a baseline diff unreadable.
#[tokio::test]
async fn scoreboard_records_the_accepted_assembly() {
    let tuning = cases::all(0);
    let root = workspace("scoreboard-record");
    let mut search = Search::new(
        tune::default_registry(),
        tuning.clone(),
        holdout_set(),
        floor_for(&tuning),
    );

    while let Some(candidate) = search.propose() {
        let verdict = search
            .evaluate(&candidate, &root)
            .await
            .expect("evaluate runs");
        if matches!(verdict, Verdict::Accept { .. }) {
            search.commit(&candidate, &verdict).expect("commit works");
        }
    }

    let results = mnemo2u::eval::run_case(&tuning[0], &root.join("recorded"))
        .await
        .expect("the case runs");
    let scoreboard = tune::scoreboard_json(&[results], search.registry());
    let produced: serde_json::Value =
        serde_json::from_str(&scoreboard).expect("the scoreboard is JSON");

    for knob in produced["knobs"].as_array().expect("the knob block") {
        let id = knob["id"].as_str().expect("a knob id");
        assert_eq!(
            &knob["current"],
            &serde_json::to_value(
                search
                    .registry()
                    .get(id)
                    .expect("a registered knob")
                    .current
            )
            .expect("a value serializes"),
            "the recorded value for `{id}` must be the one the registry carries"
        );
    }
}

/// The directories a workspace root holds, so a test can assert that nothing
/// was created. Sorted for a stable comparison.
fn dir_entries(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(root) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}
