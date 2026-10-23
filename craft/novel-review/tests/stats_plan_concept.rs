//! `stats-plan` / `stats-concept` 集成测试：与 golden 基线逐字节对照。
//!
//! 基线由参考实现（同参数、同输出布局）生成后入库
//! `tests/fixtures/expected/stats-*`。单文件报告与 SUMMARY 只内嵌 `path.name`，不含输入
//! 全路径，故 Rust 输出到临时目录后可直接按相对布局对照。

use std::fs;
use std::path::{Path, PathBuf};

use novel_review::stats::{concept, plan};

fn pin_cwd() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(dir).expect("cannot chdir to crate root");
}

/// 递归收集 `root` 下所有文件：(相对路径, 字节)，按路径排序。
fn collect_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                subdirs.push(path);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
        subdirs.sort();
        for sub in subdirs {
            walk(&sub, base, out);
        }
    }
    walk(root, root, &mut out);
    out.sort();
    out
}

/// 两棵树的文件集与内容逐字节一致。
fn diff_trees(got_root: &Path, want_root: &Path) {
    let got = collect_files(got_root);
    let want = collect_files(want_root);
    assert_eq!(
        got,
        want,
        "tree differs between {} and {}",
        got_root.display(),
        want_root.display()
    );
}

/// 递归拷贝输入树（`output_root=None` 镜像分支验证用）。
fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&path, &dst.join(path.file_name().unwrap()));
        } else {
            fs::copy(&path, dst.join(path.file_name().unwrap())).unwrap();
        }
    }
}

/// stats-plan 目录轮：`tests/fixtures/plan-novel` 整树 + `--output-root`，
/// 镜像 `*-stats` 树（含各目录 SUMMARY.md）与 golden 基线逐字节一致。
#[test]
fn stats_plan_dir_mirror_bytes_match() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan-novel")],
        &[],
        None,
        Some(out.path()),
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        out.path(),
        Path::new("tests/fixtures/expected/stats-plan-dir"),
    );
}

/// stats-plan 单文件轮：`-o` 单文件报告与基线逐字节一致。
#[test]
fn stats_plan_single_output_bytes_match() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let one = out.path().join("one.md");
    let rc = plan::run(
        &[PathBuf::from("tests/fixtures/plan-novel/N1/arc-plan/01.md")],
        &[],
        Some(&one),
        None,
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);
    let got = fs::read_to_string(&one).unwrap();
    let want = fs::read_to_string("tests/fixtures/expected/stats-plan-single.md")
        .expect("missing baseline");
    assert_eq!(got, want, "single-file report differs");
}

/// 参数面护栏：`-o`/`--output-root` 互斥、`-o` 多文件、空输入，错误信息与参考实现文案一致。
#[test]
fn stats_plan_cli_guardrails() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let err = plan::run(
        &[PathBuf::from("tests/fixtures/plan-novel")],
        &[],
        Some(out.path()),
        Some(out.path()),
        None,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("Use either --output or --output-root, not both."),
        "unexpected: {err}"
    );

    let err = plan::run(
        &[PathBuf::from("tests/fixtures/plan-novel")],
        &[],
        Some(&out.path().join("one.md")),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--output requires exactly one collected plan file."),
        "unexpected: {err}"
    );

    let err = plan::run(&[], &[], None, None, None).unwrap_err();
    assert!(
        err.to_string().contains("No input paths provided."),
        "unexpected: {err}"
    );
}

/// stats-concept 目录轮（默认排除模板）：`card-stats` 镜像树与 golden 基线逐字节一致。
///
/// 报告内嵌输入相对路径，故两侧必须用同一相对输入 `tests/fixtures/concept/cards`
/// （cwd=crate 根）；生成的 `card-stats` 属于对照产物，结束前清场。
#[test]
fn stats_concept_dir_bytes_match() {
    pin_cwd();
    let rc = concept::run(&[PathBuf::from("tests/fixtures/concept/cards")], false).unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        Path::new("tests/fixtures/concept/card-stats"),
        Path::new("tests/fixtures/expected/stats-concept-dir"),
    );
    fs::remove_dir_all("tests/fixtures/concept/card-stats").unwrap();
}

/// stats-concept `--include-templates` 轮：模板卡与 `_templates` 目录 SUMMARY 一并生成，
/// 与 golden 基线逐字节一致。
#[test]
fn stats_concept_templates_bytes_match() {
    pin_cwd();
    let rc = concept::run(&[PathBuf::from("tests/fixtures/concept-tpl/cards")], true).unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        Path::new("tests/fixtures/concept-tpl/card-stats"),
        Path::new("tests/fixtures/expected/stats-concept-templates"),
    );
    fs::remove_dir_all("tests/fixtures/concept-tpl/card-stats").unwrap();
}

/// 概念卡输入树不在 `cards` 目录下时，错误信息与参考实现文案一致。
#[test]
fn stats_concept_rejects_non_cards_tree() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let stray = out.path().join("cards-x");
    fs::create_dir_all(&stray).unwrap();
    fs::write(stray.join("a.md"), "# 状态\n- 生效\n").unwrap();
    let err = concept::run(std::slice::from_ref(&stray), false).unwrap_err();
    assert!(
        err.to_string()
            .contains("Path does not live under concept/cards:"),
        "unexpected: {err}"
    );
}

/// `summarize_runs` 边界语义：最短连续段、unclear/missing 排除、limit 截断。
#[test]
fn summarize_runs_semantics() {
    let labels = [
        "conflict",
        "conflict",
        "conflict",
        "conflict",
        "unclear",
        "conflict",
        "conflict",
        "missing",
        "procedure",
        "procedure",
        "procedure",
        "procedure",
        "procedure",
        "procedure",
    ]
    .iter()
    .copied()
    .map(String::from)
    .collect::<Vec<_>>();
    let got = plan::summarize_runs(&labels, 3, 6);
    assert_eq!(
        got,
        vec![
            "conflict x4 (#1-#4)".to_string(),
            "procedure x6 (#9-#14)".to_string(),
        ]
    );
    // 平手段长度 < min_run 不产生信号；全 unclear 输入为空。
    assert!(plan::summarize_runs(
        &["unclear".into(), "unclear".into(), "unclear".into()],
        3,
        6
    )
    .is_empty());
    assert!(plan::summarize_runs(&[], 3, 6).is_empty());
}

/// `output_root=None` 分支：镜像 `*-stats` 树写进输入树（缺省行为），
/// N3 子树与 `--output-root` 轮基线逐字节一致。
#[test]
fn stats_plan_mirror_without_output_root() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let novel = out.path().join("N3");
    copy_dir(Path::new("tests/fixtures/plan-novel/N3"), &novel);
    let rc = plan::run(std::slice::from_ref(&novel), &[], None, None, None).unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        &novel.join("arc-plan-stats"),
        Path::new("tests/fixtures/expected/stats-plan-dir/N3/arc-plan-stats"),
    );
}
