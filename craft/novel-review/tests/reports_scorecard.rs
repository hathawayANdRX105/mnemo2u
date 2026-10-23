//! `reports-scorecard` 集成测试：与 golden 基线逐字节对照
//! （`tests/fixtures/expected/reports-scorecard/{轮次}`）。
//!
//! 基线以同参数、同布局定稿，输入侧临时根目录规范化为 `tests/fixtures` 后入库。
//! 报告内嵌输入全路径（`- source: ...` 与 stdout 行），故输出树拷入临时目录运行后，
//! 按 `tests/fixtures` → 临时根 的标记替换再逐字节对照。

use std::fs;
use std::path::{Path, PathBuf};

use novel_review::reports::scorecard::{run, ScorecardOptions};

/// 把 `tests/fixtures` 钉为进程 cwd（与 stats 测试同一约定）。
fn pin_cwd() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected_variant(variant: &str) -> PathBuf {
    fixtures_root()
        .join("expected")
        .join("reports-scorecard")
        .join(variant)
}

/// 递归拷贝目录树。
fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let dst = dst.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir_all(&dst).unwrap();
            copy_dir(&entry.path(), &dst);
        } else {
            fs::copy(entry.path(), &dst).unwrap();
        }
    }
}

/// 建临时输入树：`<temp>/stats-draft`（+ 需要时 `<temp>/draft`）。
fn setup_temp(with_empty_fixture: bool) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    copy_dir(
        &fixtures_root().join("stats-draft"),
        &root.join("stats-draft"),
    );
    if with_empty_fixture {
        copy_dir(&fixtures_root().join("draft"), &root.join("draft"));
    }
    (tmp, root)
}

/// 期望侧文件字节：把入库时规范化的 `tests/fixtures` 标记还原为实际临时根。
fn expected_bytes(variant: &str, relative: &str, temp_root: &Path) -> Vec<u8> {
    let raw = fs::read(expected_variant(variant).join(relative))
        .unwrap_or_else(|err| panic!("读取期望文件失败: {}/{} ({err})", variant, relative));
    let marked = String::from_utf8(raw).unwrap();
    let restored = marked
        .replace("tests/fixtures", &temp_root.display().to_string())
        .into_bytes();
    restored
}

/// 收集 `root` 下所有文件相对路径（按字典序）。
fn list_files(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    out.sort();
    out
}

/// 一轮对照：按 variant 的期望文件集逐字节比（含 tree 与 stdout），并校验生成树无多余文件。
fn check_round(
    variant: &str,
    input: &Path,
    sample_limit: usize,
    temp_root: &Path,
) -> (i32, Vec<PathBuf>) {
    let opts = ScorecardOptions {
        paths: vec![input.to_path_buf()],
        sample_limit,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run(&opts).unwrap();
    assert_eq!(rc, 0, "round {variant}: rc 应为 0");

    // 期望树只含 `stats-draft/.../scorecards` 生成文件（stdout.txt 单独比）；
    // 生成侧 `<temp>` 下的 `draft-stats` 子树与期望文件集逐字节对照
    // （fixture 本身无既有 draft-stats 目录）。
    let want_files: Vec<String> = list_files(&expected_variant(variant))
        .into_iter()
        .filter(|f| f.contains("stats-draft"))
        .collect();
    let got_files: Vec<String> = list_files(temp_root)
        .into_iter()
        .filter(|f| f.contains("draft-stats"))
        .collect();
    assert_eq!(
        got_files, want_files,
        "round {variant}: 生成文件集与期望不一致"
    );
    for relative in &want_files {
        let got = fs::read(temp_root.join(relative)).unwrap();
        let want = expected_bytes(variant, relative, temp_root);
        assert_eq!(got, want, "round {variant}: 文件字节不一致: {relative}");
    }

    // stdout：`println!` 逐行路径 + 尾换行。
    let stdout = expected_bytes(variant, "stdout.txt", temp_root);
    let got_stdout = printed
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut got_stdout = got_stdout.into_bytes();
    got_stdout.push(b'\n');
    assert_eq!(stdout, got_stdout, "round {variant}: stdout 不一致");
    (rc, printed)
}

/// 默认轮：`novel-a/drafts` 全树（语料学习开启），scorecards 树 + SUMMARY + stdout 逐字节一致。
#[test]
fn scorecard_default_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false);
    check_round(
        "default",
        &temp.join("stats-draft/novel-a/drafts"),
        6,
        &temp,
    );
}

/// 采样上限轮：`--sample-limit 3` 下 scorecards 树 + stdout 逐字节一致。
#[test]
fn scorecard_sample_limit_3_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false);
    check_round(
        "sample3",
        &temp.join("stats-draft/novel-a/drafts"),
        3,
        &temp,
    );
}

/// 单章轮：仅 `ch01` 一章（单章镜像 SUMMARY）。
#[test]
fn scorecard_single_chapter_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false);
    check_round(
        "single",
        &temp.join("stats-draft/novel-a/drafts/ch01-信号.md"),
        6,
        &temp,
    );
}

/// 第二小说轮：`novel-b/drafts`（无语料标记，不同 story 组）。
#[test]
fn scorecard_novel_b_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false);
    check_round("novelb", &temp.join("stats-draft/novel-b/drafts"), 6, &temp);
}

/// 空输入护栏：`draft/drafts`（无 `ch` 数字章节名）→ rc=1、无打印、无输出文件
/// （stderr 文案由 CLI 轮守护，见 `scorecard_cli_round`）。
#[test]
fn scorecard_empty_input_guardrail() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(true);
    let opts = ScorecardOptions {
        paths: vec![temp.join("draft/drafts")],
        sample_limit: 6,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run(&opts).unwrap();
    assert_eq!(rc, 1);
    assert!(printed.is_empty());
    assert!(
        !temp.join("draft/draft-stats").exists(),
        "空输入不应写出任何文件"
    );
}

/// CLI 端到端轮：spawn `novel-review reports-scorecard`，stdout/退出码与 golden 基线一致；
/// 空输入轮 stderr 文案与退出码亦对齐。
#[test]
fn scorecard_cli_round() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(true);
    let bin = env!("CARGO_BIN_EXE_novel-review");

    let out = std::process::Command::new(bin)
        .args(["reports-scorecard"])
        .arg(temp.join("stats-draft/novel-a/drafts"))
        .output()
        .unwrap();
    assert!(out.status.success(), "CLI rc: {}", out.status);
    let stdout = expected_bytes("default", "stdout.txt", &temp);
    assert_eq!(out.stdout, stdout, "CLI stdout 不一致");

    let out = std::process::Command::new(bin)
        .args(["reports-scorecard"])
        .arg(temp.join("draft/drafts"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("No draft chapter files found."),
        "CLI 空输入 stderr 文案不一致: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
