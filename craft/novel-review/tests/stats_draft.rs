//! `stats-draft` 集成测试：与 golden 基线逐字节对照
//! （`tests/fixtures/expected/stats-draft/{轮次}`，同参数、同输出布局定稿）。
//! 报告只内嵌 `path.name`，不含输入
//! 全路径，故输出到临时目录后可直接按相对布局对照。

use std::fs;
use std::path::{Path, PathBuf};

use novel_review::stats::draft::{run, StatsDraftOptions};

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

fn options(
    positional: &[&str],
    output_root: Option<&Path>,
    sample_limit: usize,
    window_sizes: &[usize],
    no_corpus_learning: bool,
) -> StatsDraftOptions {
    StatsDraftOptions {
        positional: positional.iter().map(PathBuf::from).collect(),
        inputs: Vec::new(),
        output: None,
        output_root: output_root.map(|p| p.to_path_buf()),
        sample_limit,
        window_sizes: window_sizes.to_vec(),
        no_corpus_learning,
        rules: None,
    }
}

/// 默认轮：`novel-a` 整树（含 concept/chapter-plan 语料学习），
/// 单文件报告 + `draft-stats/SUMMARY.md` + `pairs`/`triples` 滚动窗口镜像树
/// 与 golden 基线逐字节一致。
#[test]
fn stats_draft_default_round_bytes_match() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let rc = run(&options(
        &["tests/fixtures/stats-draft/novel-a"],
        Some(out.path()),
        3,
        &[2, 3],
        false,
    ))
    .unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        out.path(),
        Path::new("tests/fixtures/expected/stats-draft/default"),
    );
}

/// 采样上限轮：`--sample-limit 5` 下的镜像树与基线逐字节一致。
#[test]
fn stats_draft_sample_limit_round_bytes_match() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let rc = run(&options(
        &["tests/fixtures/stats-draft/novel-a"],
        Some(out.path()),
        5,
        &[2, 3],
        false,
    ))
    .unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        out.path(),
        Path::new("tests/fixtures/expected/stats-draft/sample5"),
    );
}

/// 多目录 + 自定义窗口轮：`novel-a` + `novel-b` 双位置参数、`--window-sizes 2 3 4`，
/// 镜像树（含 `window-4` 窗口组）与基线逐字节一致。
#[test]
fn stats_draft_windows_multi_dir_round_bytes_match() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let rc = run(&options(
        &[
            "tests/fixtures/stats-draft/novel-a",
            "tests/fixtures/stats-draft/novel-b",
        ],
        Some(out.path()),
        3,
        &[2, 3, 4],
        false,
    ))
    .unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        out.path(),
        Path::new("tests/fixtures/expected/stats-draft/windows-multi"),
    );
}

/// 无语料学习轮：`novel-b`（无 concept/chapter-plan 标记）+ `--no-corpus-learning`，
/// 镜像树与基线逐字节一致。
#[test]
fn stats_draft_no_corpus_round_bytes_match() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let rc = run(&options(
        &["tests/fixtures/stats-draft/novel-b"],
        Some(out.path()),
        3,
        &[2, 3],
        true,
    ))
    .unwrap();
    assert_eq!(rc, 0);
    diff_trees(
        out.path(),
        Path::new("tests/fixtures/expected/stats-draft/no-corpus"),
    );
}

/// 参数面护栏：`-o`/`--output-root` 互斥、空输入、`-o` 多文件的退出码与文案。
#[test]
fn stats_draft_cli_guardrails() {
    pin_cwd();
    let out = tempfile::tempdir().unwrap();
    let mut opts = options(
        &["tests/fixtures/stats-draft/novel-a"],
        Some(out.path()),
        3,
        &[2, 3],
        false,
    );
    opts.output = Some(out.path().join("one.md"));
    assert_eq!(
        run(&opts).unwrap(),
        1,
        "output 与 output-root 同时提供应退出码 1"
    );

    let rc = run(&options(&[], None, 3, &[2, 3], false)).unwrap();
    assert_eq!(rc, 1, "空输入应退出码 1");

    let mut multi = options(
        &[
            "tests/fixtures/stats-draft/novel-a",
            "tests/fixtures/stats-draft/novel-b",
        ],
        None,
        3,
        &[2, 3],
        false,
    );
    multi.output = Some(out.path().join("one.md"));
    assert_eq!(run(&multi).unwrap(), 1, "-o 多于一个收集章节应退出码 1");
}
