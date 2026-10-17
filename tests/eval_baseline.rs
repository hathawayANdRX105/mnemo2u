//! The scoreboard baseline (stage doc 08 S2): the default assembly's numbers,
//! committed as an artifact and compared exactly.
//!
//! Regenerate the fixture with
//! `MNEMO_BLESS_BASELINE=1 cargo test --test eval_baseline` (it rewrites
//! `tests/fixtures/eval_baseline.json`). The run prints the artifact it
//! produced either way, so a mismatch is readable off the log.
//!
//! What is compared: the committed baseline against a fresh run of the same
//! cases through the same default assembly — the knob set, quality and cost,
//! exactly. What is not: `latency`, anywhere in the document. A wall-clock
//! sample from a shared runner is not a property of the assembly (see the
//! module doc of [`mnemo2u::tune`]), so the comparison strips every
//! `latency` field before it looks, and a run that differs only in timing is
//! not a regression. A quality or cost change is, including an improvement:
//! the point of the artifact is that moving a number is a reviewed diff.

use std::path::PathBuf;

use mnemo2u::eval::{cases, run_case, CaseResult};
use mnemo2u::tune;

/// The committed baseline, wrapped by the bless step.
const BASELINE: &str = include_str!("fixtures/eval_baseline.json");

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnemo2u-baseline-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Every `latency` field, at any depth, dropped: this function is the
/// comparison's own definition of "the same run".
fn without_latency(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "latency")
                .map(|(key, value)| (key.clone(), without_latency(value)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(without_latency).collect())
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
                        (None, Some(right)) => {
                            out.push(format!("{here}: added ({})", compact(right)))
                        }
                        (Some(left), None) => {
                            out.push(format!("{here}: removed ({})", compact(left)))
                        }
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
            (left, right) => out.push(format!("{path}: {} vs {}", compact(left), compact(right))),
        }
    }
    let mut out = Vec::new();
    walk("", expected, actual, &mut out);
    out
}

fn compact(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "?".to_string())
}

/// The frozen artifact: a fresh default-assembly run of the same cases the
/// gate replays, compared on the knob set, quality and cost.
#[tokio::test]
async fn baseline_matches() {
    let mut results: Vec<CaseResult> = Vec::new();
    for case in cases::all(0) {
        let result = run_case(&case, &workspace(&format!("baseline-{}", case.set)))
            .await
            .expect("case runs");
        assert!(
            result.passed(),
            "the baseline must be a passing run: {result:#?}"
        );
        results.push(result);
    }

    let registry = tune::default_registry();
    let scoreboard = tune::scoreboard_json(&results, &registry);
    println!("scoreboard:\n{scoreboard}");
    let produced: serde_json::Value =
        serde_json::from_str(&scoreboard).expect("the scoreboard is JSON");

    if std::env::var_os("MNEMO_BLESS_BASELINE").is_some() {
        let document = serde_json::json!({
            "generated_by": "MNEMO_BLESS_BASELINE=1 cargo test --test eval_baseline",
            "note": "quality and cost are compared exactly; latency is recorded and not compared (module doc of mnemo2u::tune)",
            "scoreboard": produced,
        });
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/eval_baseline.json");
        std::fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&document).expect("the baseline is JSON")
            ),
        )
        .expect("the baseline is written");
        println!("baseline rewritten at {}", path.display());
        return;
    }

    let baseline: serde_json::Value =
        serde_json::from_str(BASELINE).expect("the committed baseline is JSON");
    let committed = baseline
        .get("scoreboard")
        .expect("the baseline wraps the scoreboard");
    let drift = differences(&without_latency(committed), &without_latency(&produced));
    assert!(
        drift.is_empty(),
        "the default assembly drifted from the committed baseline:\n  {}\n\nscoreboard this run produced:\n{}",
        drift.join("\n  "),
        scoreboard
    );

    // A fixture that matches because nothing ran is not a baseline.
    assert_eq!(
        committed["report"]["cases"].as_array().map(Vec::len),
        Some(2),
        "the baseline covers both self-built case sets"
    );
    assert_eq!(
        produced["report"]["cases"].as_array().map(Vec::len),
        Some(2),
        "the run must cover both self-built case sets"
    );
    assert_eq!(
        produced["schema"].as_u64(),
        Some(1),
        "the artifact's schema is part of what is compared"
    );
}
