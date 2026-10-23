//! audit.draft 完整 JSON analysis 端到端对照测试。
//!
//! 基线由参考实现在 crate 根目录生成：
//! - `tests/fixtures/expected/draft-corpus.json`：
//!   `audit-draft -i tests/fixtures/draft/drafts --format json`
//! - `tests/fixtures/expected/draft-standalone.json`：
//!   `audit-draft tests/fixtures/draft/standalone.md --no-corpus-learning --format json`
//!
//! JSON 深度相等（含 int/float 类型位）；基线更新必须重跑参考实现。

use std::path::{Path, PathBuf};

use novel_review::audit::draft::{self, ReportFormat, RunOptions};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn run_json(
    opts_inputs: Vec<PathBuf>,
    output: &Path,
    no_corpus: bool,
    fail_on_warn: bool,
) -> (i32, serde_json::Value) {
    let opts = RunOptions {
        positional: Vec::new(),
        inputs: opts_inputs,
        sample_limit: 3,
        fail_on_warn,
        format: ReportFormat::Json,
        output: Some(output.to_path_buf()),
        learn_from: None,
        no_corpus_learning: no_corpus,
        rules: None,
    };
    let rc = draft::run(&opts).expect("draft run should succeed");
    let text = std::fs::read_to_string(output).expect("report file must exist");
    (rc, serde_json::from_str(&text).expect("valid json"))
}

/// 从 crate 根出发的相对路径，与基线生成时一致（source 字段依赖它）。
fn rel(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[test]
fn draft_corpus_full_analysis_golden() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("report.json");
    let expected_raw =
        std::fs::read_to_string(fixture("tests/fixtures/expected/draft-corpus.json")).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_raw).unwrap();
    let (_rc, got) = run_json(vec![rel("tests/fixtures/draft/drafts")], &out, false, false);
    assert_eq!(
        expected, got,
        "corpus-learned full analysis JSON diverges from golden baseline"
    );
}

#[test]
fn draft_standalone_no_corpus_golden() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("report.json");
    let expected_raw =
        std::fs::read_to_string(fixture("tests/fixtures/expected/draft-standalone.json")).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_raw).unwrap();
    let (_rc, got) = run_json(
        vec![rel("tests/fixtures/draft/standalone.md")],
        &out,
        true,
        false,
    );
    assert_eq!(
        expected, got,
        "standalone no-corpus analysis JSON diverges from golden baseline"
    );
}

#[test]
fn draft_fail_on_warn_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("report.json");
    let (rc_warn, _) = run_json(
        vec![rel("tests/fixtures/draft/standalone.md")],
        &out,
        true,
        true,
    );
    assert_eq!(rc_warn, 1, "warns must fail under --fail-on-warn");
    let (rc_plain, _) = run_json(
        vec![rel("tests/fixtures/draft/standalone.md")],
        &out,
        true,
        false,
    );
    assert_eq!(rc_plain, 0, "warns alone must not fail without the flag");
}
