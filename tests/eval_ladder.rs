//! The memory ladder (stage doc 07): the frozen criteria, the tiers, and the
//! marginal-gain curve they produce.
//!
//! The protocol's four steps, one test each:
//!
//! 1. the criteria are frozen, and a criteria edit is detected;
//! 2. every tier is measured;
//! 3. the curve between adjacent tiers is produced;
//! 4. a tier with no visible delta over its predecessor is flagged, so the
//!    ladder is allowed to shrink.
//!
//! The components the ladder runs against are mocks — this repository has
//! none of the models the design doc names, and a test may not download one.
//! What the tests pin is the *protocol*: the freeze, the measurement, the
//! curve, the drop rule, and the stability of the artifact.

use mnemo2u::eval::ladder::{
    expected_calibration_error, Budget, Component, ComponentKind, Criteria, Criterion,
    CriterionQuestion, Gold, JudgeAnswer, JudgeRequest, Ladder, Lang, Op, VisibilityPolicy,
    ECE_BINS,
};
use mnemo2u::eval::ladder::{ComponentProvider, ComponentRegistry, UnmeasuredRss};
use mnemo2u::eval::ladder_cases::{
    frozen_criteria, mock_registry, reference_ladder, MockBehaviour,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The ladder the shipped fixtures were produced from.
fn ladder() -> Ladder {
    reference_ladder()
}

/// The criteria the shipped fixtures were produced from.
fn criteria() -> Criteria {
    frozen_criteria()
}

/// Run the shipped ladder over the shipped criteria.
async fn run() -> mnemo2u::eval::ladder::LadderReport {
    let criteria = criteria();
    let registry = mock_registry(&criteria);
    mnemo2u::eval::ladder::run_ladder(&ladder(), &criteria, &registry, &UnmeasuredRss)
        .await
        .expect("the shipped ladder runs")
}

/// The committed ladder artifact, wrapped by the bless step.
///
/// Read at runtime rather than with `include_str!`: the artifact is
/// produced by this very test, so a checkout without it must still compile
/// — otherwise the bless step could never run to create it.
fn ladder_fixture() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/eval_ladder.json");
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "读取阶梯基线失败: {} ({error})\n生成方式: MNEMO_BLESS_LADDER=1 cargo test --test eval_ladder",
            path.display()
        )
    })
}

/// Every `latency` and `peak_rss_bytes` field, at any depth, dropped: this
/// function is the comparison's own definition of "the same run".
///
/// A wall-clock sample and a process's high-water mark are properties of the
/// machine, not of the assembly, so a run that differs only in those is not
/// a regression — the same rule the scoreboard baseline applies.
fn without_measurements(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "latency" && key.as_str() != "peak_rss_bytes")
                .map(|(key, value)| (key.clone(), without_measurements(value)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(without_measurements).collect())
        }
        other => other.clone(),
    }
}

/// The JSON paths where two documents differ, capped so a failure message
/// names the drift instead of dumping two documents side by side.
fn differences(expected: &serde_json::Value, actual: &serde_json::Value) -> Vec<String> {
    fn walk(
        path: &str,
        expected: &serde_json::Value,
        actual: &serde_json::Value,
        out: &mut Vec<String>,
    ) {
        if out.len() >= 20 {
            return;
        }
        use serde_json::Value;
        match (expected, actual) {
            (Value::Object(left), Value::Object(right)) => {
                let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
                keys.sort();
                keys.dedup();
                for key in keys {
                    let here = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    match (left.get(key), right.get(key)) {
                        (Some(left), Some(right)) => walk(&here, left, right, out),
                        (None, Some(_)) => out.push(format!("{here}: added")),
                        (Some(_), None) => out.push(format!("{here}: removed")),
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(left), Value::Array(right)) => {
                for (index, (left, right)) in left.iter().zip(right).enumerate() {
                    walk(&format!("{path}[{index}]"), left, right, out);
                }
                if left.len() != right.len() {
                    out.push(format!("{path}: {} entries vs {}", left.len(), right.len()));
                }
            }
            (left, right) if left == right => {}
            (left, right) => out.push(format!("{path}: {left} vs {right}")),
        }
    }
    let mut out = Vec::new();
    walk("", expected, actual, &mut out);
    out
}

/// The frozen criteria are frozen: the same rows hash the same, and editing
/// a criterion rotates the hash.
#[test]
fn criteria_freeze_is_hash_backed() {
    let shipped = criteria();
    assert_eq!(
        shipped.snapshot_id,
        criteria().snapshot_id,
        "the same criteria must freeze to the same hash"
    );
    assert!(
        shipped.matches_snapshot(&shipped.snapshot_id),
        "a criteria set must match its own snapshot"
    );

    // A criteria edit has to be detectable, or "the same criteria as last
    // time" is a promise rather than a comparison.
    let mut edited = criteria();
    edited.rows[0].questions[0].gold.label = "false".to_string();
    edited.snapshot_id = mnemo2u::eval::ladder::criteria_snapshot_id(&edited.rows);
    assert_ne!(
        shipped.snapshot_id, edited.snapshot_id,
        "editing a gold label must rotate the criteria hash"
    );
    assert!(
        !edited.matches_snapshot(&shipped.snapshot_id),
        "an edited criteria set must not match the frozen hash"
    );
}

/// The criteria set covers what the protocol freezes: both languages and
/// all three operations.
#[test]
fn criteria_cover_both_languages_and_every_operation() {
    let criteria = criteria();
    let languages: Vec<Lang> = criteria.rows.iter().map(|row| row.lang).collect();
    assert!(
        languages.contains(&Lang::En) && languages.contains(&Lang::Zh),
        "the criteria set must carry both an English and a Chinese row: {languages:?}"
    );
    let ops: Vec<Op> = criteria
        .rows
        .iter()
        .flat_map(|row| row.questions.iter().map(|question| question.op))
        .collect();
    for op in [Op::Noul, Op::Choice, Op::Score] {
        assert!(
            ops.contains(&op),
            "the criteria set must carry a {op:?} question"
        );
    }
}

/// A criteria set that is internally inconsistent is refused, not frozen.
#[test]
fn inconsistent_criteria_are_refused() {
    let mut rows = criteria().rows;
    // A gold label that is not one of the question's options.
    rows[0].questions[0].gold.label = "maybe".to_string();
    let error = Criteria::frozen("broken", rows).expect_err("an impossible gold label is refused");
    assert!(
        error.contains("maybe"),
        "the error must name the offending label: {error}"
    );

    // A gold distribution that does not sum to one.
    let mut rows = criteria().rows;
    rows[0].questions[0]
        .gold
        .probabilities
        .insert("true".to_string(), 0.2);
    let error = Criteria::frozen("broken", rows)
        .expect_err("a distribution that does not sum to one is refused");
    assert!(
        error.contains("sums to"),
        "the error must say the distribution does not sum to one: {error}"
    );

    // A set that covers no Chinese row is refused: the protocol freezes one
    // set per language.
    let mut rows = criteria().rows;
    rows.retain(|row| row.lang == Lang::En);
    let error = Criteria::frozen("broken", rows).expect_err("a set with no Chinese row is refused");
    assert!(
        error.contains("Chinese"),
        "the error must name the missing language: {error}"
    );
}

/// Every tier is measured: the full metric set, and the numbers the report
/// publishes.
#[tokio::test]
async fn every_tier_is_measured() {
    let report = run().await;
    assert_eq!(
        report.tiers.len(),
        4,
        "the shipped ladder has four tiers: {}",
        report.tiers.len()
    );
    for tier in &report.tiers {
        let metrics = &tier.metrics;
        assert!(
            metrics.decisions > 0,
            "tier {} measured nothing",
            tier.tier_id
        );
        assert!(
            metrics.answered > 0,
            "tier {} answered nothing: {metrics:?}",
            tier.tier_id
        );
        assert!(
            metrics.answered + metrics.abstained == metrics.decisions,
            "tier {} must account for every decision: {metrics:?}",
            tier.tier_id
        );
        assert!(
            (0.0..=1.0).contains(&metrics.accuracy),
            "tier {} has an accuracy outside [0, 1]: {}",
            tier.tier_id,
            metrics.accuracy
        );
        assert!(
            (0.0..=1.0).contains(&metrics.ece),
            "tier {} has an ece outside [0, 1]: {}",
            tier.tier_id,
            metrics.ece
        );
        assert!(
            (0.0..=1.0).contains(&metrics.flip_rate),
            "tier {} has a flip rate outside [0, 1]: {}",
            tier.tier_id,
            metrics.flip_rate
        );
        assert!(
            (0.0..=1.0).contains(&metrics.abstain_rate),
            "tier {} has an abstain rate outside [0, 1]: {}",
            tier.tier_id,
            metrics.abstain_rate
        );
        assert!(
            metrics.latency.samples > 0,
            "tier {} recorded no latency sample",
            tier.tier_id
        );
        // The per-operation breakdown has to cover every operation the
        // criteria ask about.
        for op in [Op::Noul, Op::Choice, Op::Score] {
            assert!(
                tier.metrics.per_op.contains_key(op.as_str()),
                "tier {} has no per-operation row for {op:?}",
                tier.tier_id
            );
        }
    }
}

/// The tiers differ in the way the curve needs them to: the megabytes rise
/// and the quality falls, so a step has both a cost and a gain to read.
#[tokio::test]
async fn tiers_differ_in_footprint_and_quality() {
    let report = run().await;
    let cloud = report.tier("cloud").expect("the cloud tier");
    let nano = report.tier("local-nano").expect("the nano tier");
    let mini = report.tier("local-mini").expect("the mini tier");

    assert_eq!(
        cloud.footprint_ram_mb, 0,
        "the cloud tier loads no local model"
    );
    assert!(nano.footprint_ram_mb > cloud.footprint_ram_mb);
    assert!(mini.footprint_ram_mb > nano.footprint_ram_mb);
    assert!(
        mini.metrics.accuracy > nano.metrics.accuracy,
        "the larger local judge must answer better than the small one: {} vs {}",
        mini.metrics.accuracy,
        nano.metrics.accuracy
    );
}

/// The curve: one step per adjacent pair, with the megabytes each step costs
/// and the gain it buys.
#[tokio::test]
async fn the_curve_reads_off_the_marginal_gain() {
    let report = run().await;
    assert_eq!(
        report.curve.len(),
        report.tiers.len() - 1,
        "one step per adjacent pair"
    );
    for step in &report.curve {
        let from = report
            .tiers
            .iter()
            .position(|tier| tier.tier_id == step.from_tier)
            .expect("the step starts at a tier of the ladder");
        let to = report
            .tiers
            .iter()
            .position(|tier| tier.tier_id == step.to_tier)
            .expect("the step ends at a tier of the ladder");
        assert!(
            from < to,
            "a step runs from an earlier tier to a later one: {step:?}"
        );
        if step.footprint_delta_mb == 0 {
            assert!(
                step.gain_per_mb.is_none(),
                "a step that costs nothing has no price to divide by: {step:?}"
            );
        } else {
            let gain = step
                .gain_per_mb
                .expect("a step that costs something has a gain");
            assert!(
                (gain - step.accuracy_delta / step.footprint_delta_mb as f64).abs() < 1e-12,
                "the gain per MB must be the accuracy delta over the megabytes: {step:?}"
            );
        }
    }
}

/// The drop rule: a tier that shows no visible delta over its predecessor is
/// flagged, and one that does is not.
#[tokio::test]
async fn a_tier_with_no_visible_delta_is_flagged() {
    let report = run().await;
    let invisible = report
        .curve
        .iter()
        .find(|step| step.to_tier == "local-mini-fp16")
        .expect("the last step of the curve");
    assert!(
        !invisible.visible,
        "the fp16 tier behaves like the tier before it and must not count as a visible step: {invisible:?}"
    );
    assert_eq!(
        invisible.accuracy_delta, 0.0,
        "the fp16 tier answers exactly like the tier before it: {invisible:?}"
    );
    assert!(
        invisible.footprint_delta_mb > 0,
        "the fp16 tier costs megabytes it does not spend on quality: {invisible:?}"
    );

    // And the step that does buy something is visible.
    let visible = report
        .curve
        .iter()
        .find(|step| step.to_tier == "local-mini")
        .expect("the step into the mini tier");
    assert!(
        visible.visible,
        "the step into a better judge must be visible: {visible:?}"
    );
}

/// The peak RSS is either measured or explicitly absent — never zero, which
/// would read as "this tier needed no memory".
#[tokio::test]
async fn an_unmeasured_peak_rss_is_not_zero() {
    let criteria = criteria();
    let registry = mock_registry(&criteria);
    let report = mnemo2u::eval::ladder::run_ladder(&ladder(), &criteria, &registry, &UnmeasuredRss)
        .await
        .expect("the shipped ladder runs");
    for tier in &report.tiers {
        assert_eq!(
            tier.metrics.peak_rss_bytes, None,
            "tier {} ran without a peak-RSS probe and must say so",
            tier.tier_id
        );
    }
}

/// The artifact is stable: the same ladder over the same criteria produces
/// the same assembly.
///
/// The comparison drops the measurements — the latency samples and the peak
/// RSS — because a wall-clock reading is a property of the machine and not
/// of the assembly, which is the same rule the scoreboard baseline applies.
/// The deterministic part of the artifact is compared byte for byte.
#[tokio::test]
async fn the_artifact_is_byte_stable() {
    let first = run().await;
    let second = run().await;
    assert_eq!(
        without_measurements(&serde_json::from_str::<serde_json::Value>(&first.json()).unwrap()),
        without_measurements(&serde_json::from_str::<serde_json::Value>(&second.json()).unwrap()),
        "two runs of the same ladder over the same criteria must agree byte for byte \
         on everything but the measurements"
    );
    // The measurements are still there: they are recorded, not dropped.
    for tier in &first.tiers {
        assert!(
            tier.metrics.latency.samples > 0,
            "tier {} recorded no latency sample",
            tier.tier_id
        );
    }
}

/// The committed artifact is what a fresh run produces.
#[tokio::test]
async fn the_committed_artifact_matches() {
    let report = run().await;
    let produced: serde_json::Value =
        serde_json::from_str(&report.json()).expect("the ladder report is JSON");

    if std::env::var_os("MNEMO_BLESS_LADDER").is_some() {
        let document = serde_json::json!({
            "generated_by": "MNEMO_BLESS_LADDER=1 cargo test --test eval_ladder",
            "note": "accuracy, calibration error, flip rate, abstain rate and the curve are compared exactly; latency and peak RSS are recorded and not compared (a wall-clock reading is a property of the machine, not of the assembly)",
            "ladder": produced,
        });
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/eval_ladder.json");
        std::fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&document).expect("the ladder artifact is JSON")
            ),
        )
        .expect("the ladder artifact is written");
        println!("ladder artifact rewritten at {}", path.display());
        return;
    }

    let committed: serde_json::Value =
        serde_json::from_str(&ladder_fixture()).expect("the committed ladder artifact is JSON");
    let committed = committed
        .get("ladder")
        .expect("the artifact wraps the ladder report");
    let drift = differences(
        &without_measurements(committed),
        &without_measurements(&produced),
    );
    assert!(
        drift.is_empty(),
        "the ladder artifact drifted from the committed one:\n  {}\n\nthis run produced:\n{}",
        drift.join("\n  "),
        report.json()
    );
}

/// The calibration error is the definition the judgment pipeline uses.
#[test]
fn the_calibration_error_is_the_pipeline_definition() {
    // A perfectly calibrated, perfectly correct sample: no error.
    assert_eq!(expected_calibration_error(&[(1.0, true)], ECE_BINS), 0.0);
    // A perfectly confident and perfectly wrong sample: the whole gap.
    assert_eq!(expected_calibration_error(&[(1.0, false)], ECE_BINS), 1.0);
    // A sample whose confidence is its accuracy: no error.
    assert_eq!(
        expected_calibration_error(&[(0.5, true), (0.5, false)], ECE_BINS),
        0.0
    );
    // An empty sample is not an error, it is nothing to measure.
    assert_eq!(expected_calibration_error(&[], ECE_BINS), 0.0);
    // Two samples in the same bin, one right and one wrong: the bin's mean
    // confidence is 1.0 and its accuracy is 0.5, so the bin contributes its
    // whole weight times a gap of half.
    let ece = expected_calibration_error(&[(1.0, false), (1.0, true)], ECE_BINS);
    assert!((ece - 0.5).abs() < 1e-9, "expected 0.5, got {ece}");
    // The gap is not one-sided: a sample that is certain and wrong and a
    // sample that is certain it is right both sit at the far end of the
    // scale, and each contributes the whole gap.
    let ece = expected_calibration_error(&[(1.0, false), (0.0, true)], ECE_BINS);
    assert!((ece - 1.0).abs() < 1e-9, "expected 1.0, got {ece}");
}

/// A component that answers by position flips when the option order
/// changes, and one that reads the content does not. The probe has to be
/// able to tell them apart, or the flip rate measures nothing.
#[tokio::test]
async fn the_option_order_probe_separates_position_from_content() {
    // The option ids are words rather than letters, so that the state text
    // cannot contain one of them as a substring of an unrelated word — a
    // content-reading component that matched "a" inside "answer" would read
    // as positional, and the probe would be measuring the fixture instead of
    // the component.
    let question = CriterionQuestion {
        question_id: "q".to_string(),
        op: Op::Choice,
        instructions: "which one?".to_string(),
        criteria: vec![
            Criterion {
                option_id: "alpha".to_string(),
                description: "the first".to_string(),
            },
            Criterion {
                option_id: "beta".to_string(),
                description: "the second".to_string(),
            },
            Criterion {
                option_id: "gamma".to_string(),
                description: "the third".to_string(),
            },
        ],
        gold: Gold {
            label: "gamma".to_string(),
            probabilities: BTreeMap::from([
                ("alpha".to_string(), 0.1),
                ("beta".to_string(), 0.1),
                ("gamma".to_string(), 0.8),
            ]),
        },
    };
    let request = JudgeRequest {
        row_id: "r".to_string(),
        question_id: "q".to_string(),
        op: Op::Choice,
        lang: Lang::En,
        state: "the result is gamma".to_string(),
        instructions: question.instructions.clone(),
        criteria: question.criteria.clone(),
    };

    // A component that always takes the first option: the order probe sees
    // a flip, because the reversed list starts with a different option.
    struct Positional;
    #[async_trait::async_trait]
    impl ComponentProvider for Positional {
        fn component_id(&self) -> &str {
            "positional"
        }
        async fn answer(&self, request: &JudgeRequest) -> Result<JudgeAnswer, String> {
            Ok(JudgeAnswer {
                choice: Some(request.criteria[0].option_id.clone()),
                noul: None,
                score: None,
                probabilities: BTreeMap::new(),
                confidence: Some(0.9),
                abstain: false,
            })
        }
    }

    // A component that reads the state: the order does not change its
    // answer, so the probe sees no flip.
    struct ContentReading;
    #[async_trait::async_trait]
    impl ComponentProvider for ContentReading {
        fn component_id(&self) -> &str {
            "content"
        }
        async fn answer(&self, request: &JudgeRequest) -> Result<JudgeAnswer, String> {
            let chosen = request
                .criteria
                .iter()
                .find(|criterion| request.state.contains(&criterion.option_id))
                .map(|criterion| criterion.option_id.clone())
                .unwrap_or_default();
            Ok(JudgeAnswer {
                choice: Some(chosen),
                noul: None,
                score: None,
                probabilities: BTreeMap::new(),
                confidence: Some(0.9),
                abstain: false,
            })
        }
    }

    let mut registry = ComponentRegistry::new();
    registry.register(Arc::new(Positional));
    registry.register(Arc::new(ContentReading));

    let positional = registry.get("positional").expect("registered");
    let first = positional.answer(&request).await.expect("answers");
    let reversed = JudgeRequest {
        criteria: request.criteria.iter().rev().cloned().collect(),
        ..request.clone()
    };
    let second = positional.answer(&reversed).await.expect("answers");
    assert_ne!(
        first.choice, second.choice,
        "a positional component must answer differently when the order is reversed"
    );

    let content = registry.get("content").expect("registered");
    let first = content.answer(&request).await.expect("answers");
    let second = content.answer(&reversed).await.expect("answers");
    assert_eq!(
        first.choice, second.choice,
        "a content-reading component must answer the same when the order is reversed"
    );
}

/// A tier that declares a component it does not load is refused, and so is
/// a tier whose declaration and components disagree.
#[test]
fn a_tier_whose_declaration_disagrees_with_its_components_is_refused() {
    let mut tiers = reference_ladder().tiers;
    // The nano tier says it adds the mini judge, but it loads the nano one.
    tiers[1].adds = vec!["judge-mini".to_string()];
    let error = Ladder::frozen("broken", tiers, VisibilityPolicy::default())
        .expect_err("a tier that disagrees with itself is refused");
    assert!(
        error.contains("judge-mini"),
        "the error must name the offending component: {error}"
    );
}

/// A tier that does not fit its budget is refused.
#[test]
fn a_tier_over_its_budget_is_refused() {
    let mut tiers = reference_ladder().tiers;
    tiers[1].budget = Budget {
        ram_mb: 1,
        disk_mb: 1,
    };
    let error = Ladder::frozen("broken", tiers, VisibilityPolicy::default())
        .expect_err("a tier over its budget is refused");
    assert!(
        error.contains("does not fit its budget"),
        "the error must say the tier does not fit: {error}"
    );
}

/// The visibility policy is frozen with the ladder, so the verdict cannot be
/// tuned after the curve is read.
#[test]
fn the_visibility_policy_is_frozen_with_the_ladder() {
    let ladder = ladder();
    assert_eq!(
        ladder.snapshot_id,
        reference_ladder().snapshot_id,
        "the same ladder must freeze to the same hash"
    );
    let mut widened = reference_ladder().tiers;
    widened[1].budget.ram_mb += 1;
    let other = Ladder::frozen("reference-ladder", widened, VisibilityPolicy::default())
        .expect("a ladder is consistent");
    assert_ne!(
        ladder.snapshot_id, other.snapshot_id,
        "changing a tier's budget must rotate the ladder hash"
    );

    // And the policy itself is part of the hash: a threshold moved after the
    // fact is a threshold that was not frozen.
    let mut relaxed = reference_ladder().tiers;
    let policy = VisibilityPolicy {
        min_accuracy_delta: 0.5,
        min_ece_delta: 0.5,
    };
    let other = Ladder::frozen("reference-ladder", relaxed.clone(), policy).expect("consistent");
    assert_ne!(
        ladder.snapshot_id, other.snapshot_id,
        "a different visibility policy must rotate the ladder hash"
    );
    relaxed.clear();
}

/// The mock's quality is its behaviour, not its name: two components that
/// behave the same answer the same, which is what lets a tier be a
/// strictly-wider build of the tier before it.
#[tokio::test]
async fn the_mock_answers_by_behaviour_not_by_name() {
    let criteria = criteria();
    let registry = mock_registry(&criteria);
    let mini = registry.get("judge-mini").expect("the mini judge");
    let fp16 = registry.get("judge-mini-fp16").expect("the fp16 judge");
    let request = JudgeRequest {
        row_id: "en-0001".to_string(),
        question_id: "en-0001-choice".to_string(),
        op: Op::Choice,
        lang: Lang::En,
        state: criteria.rows[0].state.clone(),
        instructions: "Which rotation window was settled?".to_string(),
        criteria: criteria.rows[0].questions[1].criteria.clone(),
    };
    assert_eq!(
        mini.answer(&request).await.expect("answers"),
        fp16.answer(&request).await.expect("answers"),
        "two components with the same behaviour must answer the same"
    );
}

/// A mock that does not know the answer still answers, and its answer may
/// be wrong — that is what the accuracy axis measures.
#[tokio::test]
async fn a_mock_answers_even_when_it_does_not_know() {
    let criteria = criteria();
    // A component that never knows and never declines: whatever it answers
    // is a guess, and a guess is allowed to be wrong. The thresholds are
    // what make it such a component, so the test does not depend on which
    // questions a shipped component happens to draw.
    let mut registry = ComponentRegistry::new();
    registry.register(Arc::new(mnemo2u::eval::ladder_cases::MockComponent::new(
        "judge-guesser",
        MockBehaviour {
            skill: 0.0,
            abstain_rate: 0.0,
            known_confidence: 0.9,
            guessed_confidence: 0.3,
        },
        &criteria,
    )));
    let guesser = registry.get("judge-guesser").expect("registered");
    let request = JudgeRequest {
        row_id: "en-0001".to_string(),
        question_id: "en-0001-choice".to_string(),
        op: Op::Choice,
        lang: Lang::En,
        state: criteria.rows[0].state.clone(),
        instructions: "Which rotation window was settled?".to_string(),
        criteria: criteria.rows[0].questions[1].criteria.clone(),
    };
    let answer = guesser.answer(&request).await.expect("answers");
    assert!(
        !answer.abstain,
        "the mock must answer a question it can attempt"
    );
    let chosen = answer.choice.expect("a choice question gets a choice");
    assert!(
        request
            .criteria
            .iter()
            .any(|criterion| criterion.option_id == chosen),
        "the guess must be one of the options offered: {chosen}"
    );
}

/// A component behaviour that abstains on everything is refused by the
/// runner, because a tier that answered nothing has no accuracy to report.
#[tokio::test]
async fn a_tier_that_answers_nothing_is_refused() {
    let criteria = criteria();
    let mut registry = ComponentRegistry::new();
    registry.register(Arc::new(mnemo2u::eval::ladder_cases::MockComponent::new(
        "judge-cloud",
        MockBehaviour {
            skill: 0.9,
            abstain_rate: 1.0,
            known_confidence: 0.9,
            guessed_confidence: 0.3,
        },
        &criteria,
    )));
    let error = mnemo2u::eval::ladder::run_ladder(
        &reference_ladder(),
        &criteria,
        &registry,
        &UnmeasuredRss,
    )
    .await
    .expect_err("a tier that answered nothing is refused");
    assert!(
        error.contains("answered none"),
        "the error must say the tier answered nothing: {error}"
    );
}

/// A tier's declared footprint is the sum of its components' — the MB axis
/// has to be the same number for every tier.
#[test]
fn the_footprint_is_the_sum_of_the_components() {
    for tier in reference_ladder().tiers {
        let ram: u64 = tier
            .components
            .iter()
            .map(|component| component.ram_mb)
            .sum();
        let disk: u64 = tier
            .components
            .iter()
            .map(|component| component.disk_mb)
            .sum();
        assert_eq!(tier.footprint_ram_mb(), ram);
        assert_eq!(tier.footprint_disk_mb(), disk);
        assert!(
            ram <= tier.budget.ram_mb,
            "tier {} does not fit its budget",
            tier.tier_id
        );
    }
}

/// The markdown report carries the curve, and the curve carries the tiers'
/// added and removed components.
#[tokio::test]
async fn the_markdown_report_carries_the_curve() {
    let report = run().await;
    let markdown = report.markdown();
    assert!(markdown.contains("# Memory ladder report"));
    assert!(markdown.contains("## Marginal gain per MB"));
    assert!(markdown.contains("## Tiers"));
    for step in &report.curve {
        assert!(
            markdown.contains(&format!("{} -> {}", step.from_tier, step.to_tier)),
            "the markdown must carry the step {step:?}"
        );
    }
    // The components a tier adds are in the report: "this tier is more than
    // the one before it" has to be readable off the artifact.
    let nano = report.tier("local-nano").expect("the nano tier");
    assert_eq!(nano.added, vec!["judge-nano".to_string()]);
    assert!(
        markdown.contains("judge-nano"),
        "the markdown must name the component the tier adds"
    );
}

/// A tier that loads a component the registry does not have is refused, so a
/// typo in a tier definition is a red run rather than a silent zero.
#[tokio::test]
async fn a_missing_component_is_refused() {
    let criteria = criteria();
    let registry = ComponentRegistry::new();
    let error = mnemo2u::eval::ladder::run_ladder(
        &reference_ladder(),
        &criteria,
        &registry,
        &UnmeasuredRss,
    )
    .await
    .expect_err("a tier with no registered component is refused");
    assert!(
        error.contains("no provider registered"),
        "the error must say the component is missing: {error}"
    );
}

/// A tier that loads two judges is refused: the runner could not tell which
/// one answers.
#[test]
fn a_tier_with_two_judges_is_refused() {
    let mut tiers = reference_ladder().tiers;
    tiers[1].components.push(Component {
        component_id: "judge-second".to_string(),
        kind: ComponentKind::Judge,
        ram_mb: 1,
        disk_mb: 1,
    });
    let error = Ladder::frozen("broken", tiers, VisibilityPolicy::default())
        .expect_err("a tier with two judges is refused");
    assert!(
        error.contains("cannot tell which one answers"),
        "the error must say the tier is ambiguous: {error}"
    );
}

/// A row with no questions cannot be frozen: it would be a row that measures
/// nothing while inflating the row count.
#[test]
fn a_row_with_no_questions_is_refused() {
    let mut rows = criteria().rows;
    rows[0].questions.clear();
    let error = Criteria::frozen("broken", rows).expect_err("an empty row is refused");
    assert!(
        error.contains("no questions"),
        "the error must say the row has no questions: {error}"
    );
}
