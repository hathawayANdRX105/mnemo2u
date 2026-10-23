//! `audit-plan` / `audit-concept` 集成测试：与 golden 基线逐字节/深比较。
//!
//! 基线以相对输入路径（`tests/fixtures/...`）定稿，故测试先 pin cwd 到仓库根。

use novel_review::audit::{concept, plan};
use novel_review::config;
use std::path::{Path, PathBuf};

fn pin_cwd() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(dir).expect("cannot chdir to crate root");
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/expected/{name}"))
        .unwrap_or_else(|e| panic!("missing expected fixture {name}: {e}"))
}

const PLAN_FILES: &[&str] = &[
    "arc-plan.md",
    "chapter-plan.md",
    "clean-plan.md",
    "story-plan.md",
];

/// 逐文件的 markdown 报告与 expected/plan-out-dir 逐字节一致。
#[test]
fn plan_per_file_markdown_report_bytes_match() {
    pin_cwd();
    let cfg = config::load_rules(&config::default_rules_path()).unwrap();
    let engine = plan::PlanEngine::new(&cfg.plan, &cfg.paths).expect("load config");
    for name in PLAN_FILES {
        let path = Path::new("tests/fixtures/plan").join(name);
        let (plan_type, warnings) = plan::audit_file(&engine, &path).unwrap();
        let got = plan::format_report(&path, &plan_type, &warnings);
        let want = std::fs::read_to_string(format!("tests/fixtures/expected/plan-out-dir/{name}"))
            .unwrap()
            .trim_end_matches('\n')
            .to_string();
        assert_eq!(got, want, "markdown report differs for {name}");
    }
}

/// 目录目标 JSON 报告与 golden 基线深度一致（含顺序与 source 字符串）；
/// `-i` 输入合并（仅输入、无位置参数）可用。
#[test]
fn plan_dir_json_reports_deep_equal() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let json_path = out.path().join("plan.json");

    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan")],
        &[],
        plan::OutputFormat::Json,
        false,
        Some(&json_path),
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);
    let got: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    let want: serde_json::Value = serde_json::from_str(&expected("plan-dir.stdout.json")).unwrap();
    assert_eq!(got, want, "dir json report differs");

    let single = PathBuf::from("tests/fixtures/plan/clean-plan.md");
    let rc = plan::run(
        &[],
        std::slice::from_ref(&single),
        plan::OutputFormat::Text,
        true,
        Some(&out.path().join("input-only.txt")),
        None,
    )
    .unwrap();
    assert_eq!(rc, 0, "input-only clean file should exit 0");
}

/// `run` 写文件模式：markdown 目录 / 单文件 json / 单文件 text 与 expected 一致；
/// `--fail-on-warn` 的 rc（有警告输入=1，干净输入=0）。
#[test]
fn plan_run_writes_files_and_return_codes() {
    pin_cwd();
    let out_dir = tempfile::tempdir().unwrap();

    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan")],
        &[],
        plan::OutputFormat::Markdown,
        false,
        Some(out_dir.path()),
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);
    for name in PLAN_FILES {
        let got = std::fs::read(out_dir.path().join(name)).unwrap();
        let want = std::fs::read(format!("tests/fixtures/expected/plan-out-dir/{name}")).unwrap();
        assert_eq!(got, want, "written markdown file differs for {name}");
    }

    let json_out = out_dir.path().join("single.json");
    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan/clean-plan.md")],
        &[],
        plan::OutputFormat::Json,
        false,
        Some(&json_out),
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);
    let got: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&json_out).unwrap()).unwrap();
    let want: serde_json::Value = serde_json::from_str(&expected("plan-json-single.json")).unwrap();
    assert_eq!(got, want, "single-file json differs");

    let txt_out = out_dir.path().join("clean.txt");
    plan::run(
        &[PathBuf::from("tests/fixtures/plan/clean-plan.md")],
        &[],
        plan::OutputFormat::Text,
        false,
        Some(&txt_out),
        None,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&txt_out).unwrap(),
        std::fs::read("tests/fixtures/expected/plan-clean.txt").unwrap(),
        "text report differs"
    );

    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan")],
        &[],
        plan::OutputFormat::Text,
        true,
        Some(&out_dir.path().join("a.txt")),
        None,
    )
    .unwrap();
    assert_eq!(rc, 1, "warn input + fail_on_warn should exit 1");
    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan/clean-plan.md")],
        &[],
        plan::OutputFormat::Text,
        true,
        Some(&out_dir.path().join("b.txt")),
        None,
    )
    .unwrap();
    assert_eq!(rc, 0, "clean input + fail_on_warn should exit 0");
}

/// 概念卡报告与 expected stdout 逐字节一致（含/不含 `--include-templates`）；
/// `run` 的 rc：有警告目录=1，干净卡=0。
#[test]
fn concept_reports_and_rc_match_expected() {
    pin_cwd();
    let cards = PathBuf::from("tests/fixtures/concept/cards");
    for (include_templates, name) in [
        (false, "concept-cards.stdout.md"),
        (true, "concept-cards-templates.stdout.md"),
    ] {
        let targets = concept::iter_targets(std::slice::from_ref(&cards), include_templates);
        let mut got = String::new();
        for t in &targets {
            let warnings = concept::audit_card(t).unwrap();
            got.push_str(&concept::format_report(t, &warnings));
            got.push_str("\n\n");
        }
        assert_eq!(
            got,
            expected(name),
            "concept stdout differs (include_templates={include_templates})"
        );
    }

    let rc = concept::run(std::slice::from_ref(&cards), false).unwrap();
    assert_eq!(rc, 1, "card dir has warnings -> rc 1");
    let clean = cards.join("characters").join("张远.md");
    let rc = concept::run(&[clean], false).unwrap();
    assert_eq!(rc, 0, "clean card -> rc 0");
}

/// 干净概念卡的 markdown 报告与 expected 逐字节一致。
#[test]
fn concept_clean_card_report_bytes_match() {
    pin_cwd();
    let path = PathBuf::from("tests/fixtures/concept/cards/characters/张远.md");
    let warnings = concept::audit_card(&path).unwrap();
    assert!(warnings.is_empty(), "张远 card should be clean");
    let got = concept::format_report(&path, &warnings);
    let want = expected("concept-clean.stdout.md")
        .trim_end_matches('\n')
        .to_string();
    assert_eq!(got, want, "clean concept report differs");
}
