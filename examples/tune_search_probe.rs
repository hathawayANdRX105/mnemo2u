//! Smoke probe for the tuning search loop (stage doc 08 S3).
//!
//! Scores the default assembly on both case sets — the two self-built cases as
//! the tuning set, the two external slices as the holdout — then drives one
//! full coordinate-descent search over the injectable knobs: `propose`,
//! `evaluate`, `commit` on an accept, until `propose` reports no direction
//! left. Run by CI (`.github/workflows/ci.yml`, "Smoke (tune search probe)") —
//! it exits non-zero when the default assembly stops passing on either set,
//! when the search does not terminate, or when an evaluation errors. A search
//! that accepts nothing is still a green run, with a line that says so: the
//! probe reports what the search decided and never fakes a gain to look green.

use std::path::Path;

use mnemo2u::eval::{cases, external, run_case, run_case_with, Case, CaseResult};
use mnemo2u::query::facts::RecallKnobs;
use mnemo2u::tune;
use mnemo2u::tune::search::{Search, Verdict};
use mnemo2u::tune::KnobRegistry;

/// How many single-knob directions one search may propose before the probe
/// calls it non-terminating. A full pass is one direction per knob per side
/// (seven knobs, two sides), so a few passes is already more than a converging
/// search needs: reaching the cap means `propose` stopped converging, not that
/// the search is merely long. Convergence itself is what ends the loop.
const MAX_STEPS: usize = 32;

#[tokio::main]
async fn main() -> Result<(), String> {
    // One temp root for the whole probe, a subdirectory per run: the CI
    // runner's `/tmp` is the only thing this probe writes to.
    let root =
        std::env::temp_dir().join(format!("mnemo2u-tune-search-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    // 1. The two sets exactly as the search sees them: scale 0, the fixtures
    // already in the repo, nothing added.
    let tuning = cases::all(0);
    let holdout = external::external_all(0, &external::fixture_dir())?;
    if tuning.is_empty() || holdout.is_empty() {
        return Err("a case set came back empty; a probe with no case says nothing".to_string());
    }
    let tuning_queries = count_queries(&tuning);
    let holdout_queries = count_queries(&holdout);
    if tuning_queries == 0 || holdout_queries == 0 {
        return Err(format!(
            "a case set has no query (tuning {tuning_queries}, holdout {holdout_queries}), so a \
             noise floor over it is undefined"
        ));
    }
    // The noise floor is one query's weight in the tuning set — the largest
    // quality change a single query's outcome can make — so a gain has to move
    // at least two queries to clear it (stage doc 08 S3). Derived here rather
    // than hardcoded: a set that grows re-derives its own floor.
    let noise_floor = 1.0 / tuning_queries as f64;
    println!(
        "tune search probe: {:<12} -> {} case(s), {} query/queries",
        "tuning set",
        tuning.len(),
        tuning_queries
    );
    println!(
        "tune search probe: {:<12} -> {} case(s), {} query/queries",
        "holdout",
        holdout.len(),
        holdout_queries
    );
    println!(
        "tune search probe: {:<12} -> {noise_floor} (1.0 / {tuning_queries} tuning queries)",
        "noise floor"
    );

    // 2. The default assembly on both sets, summarized by the scoreboard. The
    // baseline is what a search step is measured against, so a case that stops
    // passing here is red before any knob moves.
    let registry = tune::default_registry();
    let baseline = run_all(&tuning, &root, "default-tuning", None).await?;
    print_set("default tuning", &baseline, &registry)?;
    check_passed("the default assembly", "the tuning set", &baseline)?;
    let baseline_holdout = run_all(&holdout, &root, "default-holdout", None).await?;
    print_set("default holdout", &baseline_holdout, &registry)?;
    check_passed("the default assembly", "the holdout set", &baseline_holdout)?;

    // 3. One full search: propose, evaluate, commit on an accept, repeat until
    // there is no direction left. An `evaluate` that returns `Err` ends the
    // probe here — a search that cannot score a candidate cannot be green.
    let mut search = Search::new(registry, tuning.clone(), holdout.clone(), noise_floor);
    println!(
        "tune search probe: {:<12} -> {} knob(s) registered, {} accepted at the start",
        "search",
        search.registry().knobs().len(),
        search.accepted().len()
    );
    let mut proposed = 0usize;
    let mut accepted = 0usize;
    while let Some(step) = search.propose() {
        proposed += 1;
        if proposed > MAX_STEPS {
            return Err(format!(
                "the search did not terminate: {proposed} single-knob directions proposed past \
                 the {MAX_STEPS}-step cap"
            ));
        }
        println!(
            "tune search probe: {:<12} -> {}: {:?} -> {:?}",
            format!("step {proposed}"),
            step.knob_id,
            step.from,
            step.to,
        );
        let verdict = search.evaluate(&step, &root).await?;
        println!(
            "tune search probe: {:<12} -> {}",
            format!("verdict {proposed}"),
            verdict_line(&verdict)
        );
        if matches!(verdict, Verdict::Accept { .. }) {
            search.commit(&step, &verdict)?;
            accepted += 1;
        }
    }

    // 4. What the search accepted (the ratchet), and what the accepted
    // assembly scores on the tuning set.
    println!(
        "tune search probe: {:<12} -> {} of {} proposed direction(s) accepted",
        "search", accepted, proposed
    );
    for step in search.accepted() {
        println!(
            "tune search probe: {:<12} -> {}: {:?} -> {:?}",
            "ratchet", step.knob_id, step.from, step.to,
        );
    }
    if accepted == 0 {
        if proposed == 0 {
            println!(
                "tune search probe: {:<12} -> no accepted step: the search proposed no direction \
                 at all, so the default assembly already stands",
                "search"
            );
        } else {
            println!(
                "tune search probe: {:<12} -> no accepted step: all {proposed} proposed \
                 direction(s) were rejected, so the default assembly stands and no gain was faked",
                "search"
            );
        }
    }
    let knobs = search
        .registry()
        .recall_knobs()
        .map_err(|error| error.to_string())?;
    let accepted_results = run_all(&tuning, &root, "accepted-tuning", Some(&knobs)).await?;
    print_set("accepted tuning", &accepted_results, search.registry())?;
    Ok(())
}

/// Every case of a set, each into its own subdirectory of `root`, tagged by
/// `label` so two assemblies never share a store. `None` runs the default
/// assembly through the pinned [`run_case`]; `Some` injects a registry's
/// values through the seam.
async fn run_all(
    cases: &[Case],
    root: &Path,
    label: &str,
    knobs: Option<&RecallKnobs>,
) -> Result<Vec<CaseResult>, String> {
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let workspace = root.join(format!("{label}-{}", case.case_id));
        let _ = std::fs::remove_dir_all(&workspace);
        let result = match knobs {
            Some(knobs) => run_case_with(case, &workspace, knobs).await?,
            None => run_case(case, &workspace).await?,
        };
        results.push(result);
    }
    Ok(results)
}

/// The scoreboard of one set, summarized the way `examples/tune_probe.rs`
/// reads it: the artifact is the contract, the probe only quotes the totals
/// out of the parsed JSON.
fn print_set(label: &str, results: &[CaseResult], registry: &KnobRegistry) -> Result<(), String> {
    let scoreboard = tune::scoreboard_json(results, registry);
    let produced: serde_json::Value =
        serde_json::from_str(&scoreboard).map_err(|error| error.to_string())?;
    let report = &produced["report"];
    println!(
        "tune search probe: {:<12} -> recall={} miss={} wrong={} embed_calls={} schema={}",
        label,
        report["quality"]["recall_at_k"],
        report["quality"]["miss_rate"],
        report["quality"]["wrong_evidence_rate"],
        report["cost"]["embed_calls"],
        produced["schema"]
    );
    Ok(())
}

/// Every case of a set met its frozen conditions. The failing case ids are
/// named, because "a case failed" without one is not actionable in a log.
fn check_passed(what: &str, which: &str, results: &[CaseResult]) -> Result<(), String> {
    let failed: Vec<&str> = results
        .iter()
        .filter(|result| !result.passed())
        .map(|result| result.case_id.as_str())
        .collect();
    if failed.is_empty() {
        return Ok(());
    }
    Err(format!("{what} stopped passing on {which}: {failed:?}"))
}

/// The query count of a case set — the denominator of the noise floor.
fn count_queries(cases: &[Case]) -> usize {
    cases.iter().map(|case| case.queries.len()).sum()
}

/// One verdict as the line the probe prints. Matched by hand rather than
/// printed as `Debug`: the wording is what a human reads in a CI log.
fn verdict_line(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Accept { gain } => format!("accept (gain {gain})"),
        Verdict::RejectNoiseFloor { gain } => {
            format!("reject: inside the noise floor (gain {gain})")
        }
        Verdict::RejectRegression { reason } => format!("reject: regression ({reason})"),
        Verdict::RejectOutOfBounds { reason } => format!("reject: out of bounds ({reason})"),
    }
}
