//! audit.draft text/markdown 渲染端到端对照测试。
//!
//! 基线由参考实现在 crate 根目录生成：
//! - `tests/fixtures/expected/draft-standalone.stdout.txt` / `.stdout.md`：
//!   `audit-draft tests/fixtures/draft/standalone.md --no-corpus-learning --format text|markdown`（stdout）
//! - `tests/fixtures/expected/draft-corpus.stdout.txt` / `.stdout.md`：
//!   `audit-draft tests/fixtures/draft/drafts --format text|markdown`（stdout，带语料学习）
//! - `tests/fixtures/expected/draft-standalone.txt` / `.md`：
//!   同上单文件输入 `-o <file>` 写盘（报告 + 末尾换行）
//! - `tests/fixtures/expected/draft-corpus/0001-信号.{txt,md}`、
//!   `0002-断桥.{txt,md}`：同上目录输入 `-o <dir>` 写盘（每稿一份，按 stem 命名）
//!
//! 基线更新必须重跑参考实现。

use std::path::{Path, PathBuf};
use std::process::Command;

use novel_review::audit::draft::{self, ReportFormat, RunOptions};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// 从 crate 根出发的相对路径，与基线生成时一致（source 字段依赖它）。
fn rel(path: &str) -> PathBuf {
    PathBuf::from(path)
}

/// 在 crate 根目录运行 novel-review 二进制，返回（退出码、stdout 字节）。
fn run_bin(args: &[&str]) -> (i32, Vec<u8>) {
    let out = Command::new(env!("CARGO_BIN_EXE_novel-review"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("novel-review bin must run");
    (out.status.code().unwrap_or(-1), out.stdout)
}

/// stdout 轮次：字节对照基线。
fn expect_stdout(args: &[&str], expected: &str) {
    let (rc, stdout) = run_bin(args);
    let want =
        std::fs::read(fixture(expected)).unwrap_or_else(|e| panic!("读取 {expected} 失败: {e}"));
    assert_eq!(rc, 0, "stdout 轮次应退出 0");
    assert_eq!(stdout, want, "{expected} 的 stdout 与基线不一致");
}

#[test]
fn draft_standalone_text_stdout_golden() {
    expect_stdout(
        &[
            "audit-draft",
            "tests/fixtures/draft/standalone.md",
            "--no-corpus-learning",
            "--format",
            "text",
        ],
        "tests/fixtures/expected/draft-standalone.stdout.txt",
    );
}

#[test]
fn draft_standalone_markdown_stdout_golden() {
    expect_stdout(
        &[
            "audit-draft",
            "tests/fixtures/draft/standalone.md",
            "--no-corpus-learning",
            "--format",
            "markdown",
        ],
        "tests/fixtures/expected/draft-standalone.stdout.md",
    );
}

#[test]
fn draft_corpus_text_stdout_golden() {
    expect_stdout(
        &[
            "audit-draft",
            "tests/fixtures/draft/drafts",
            "--format",
            "text",
        ],
        "tests/fixtures/expected/draft-corpus.stdout.txt",
    );
}

#[test]
fn draft_corpus_markdown_stdout_golden() {
    expect_stdout(
        &[
            "audit-draft",
            "tests/fixtures/draft/drafts",
            "--format",
            "markdown",
        ],
        "tests/fixtures/expected/draft-corpus.stdout.md",
    );
}

#[test]
fn draft_explicit_learn_from_stdout_golden() {
    expect_stdout(
        &[
            "audit-draft",
            "tests/fixtures/draft/drafts/0001-信号.md",
            "--learn-from",
            "tests/fixtures/draft/concept/cards",
            "--format",
            "text",
        ],
        "tests/fixtures/expected/draft-learn-from.stdout.txt",
    );
}

/// `--fail-on-warn` 退出码（有 warn → 1；无 flag → 0）。
#[test]
fn draft_fail_on_warn_exit_code_text() {
    let (rc_warn, _) = run_bin(&[
        "audit-draft",
        "tests/fixtures/draft/standalone.md",
        "--no-corpus-learning",
        "--fail-on-warn",
    ]);
    assert_eq!(rc_warn, 1, "有警告且 --fail-on-warn 时应退出 1");
    let (rc_plain, _) = run_bin(&[
        "audit-draft",
        "tests/fixtures/draft/standalone.md",
        "--no-corpus-learning",
    ]);
    assert_eq!(rc_plain, 0, "无 flag 时仅告警不失败");
}

fn run_write(inputs: Vec<PathBuf>, format: ReportFormat, no_corpus: bool, output: &Path) -> i32 {
    let opts = RunOptions {
        positional: Vec::new(),
        inputs,
        sample_limit: 3,
        fail_on_warn: false,
        format,
        output: Some(output.to_path_buf()),
        learn_from: None,
        no_corpus_learning: no_corpus,
        rules: None,
    };
    draft::run(&opts).expect("draft run should succeed")
}

/// 单文件 `-o` 写盘：内容与基线逐字节一致。
#[test]
fn draft_single_file_write_golden() {
    let tmp = tempfile::tempdir().unwrap();
    let out_txt = tmp.path().join("report.txt");
    let rc = run_write(
        vec![rel("tests/fixtures/draft/standalone.md")],
        ReportFormat::Text,
        true,
        &out_txt,
    );
    assert_eq!(rc, 0);
    let got = std::fs::read(&out_txt).expect("text report file must exist");
    let want = std::fs::read(fixture("tests/fixtures/expected/draft-standalone.txt")).unwrap();
    assert_eq!(got, want, "单文件 -o text 写盘与基线不一致");

    let out_md = tmp.path().join("report.md");
    let rc = run_write(
        vec![rel("tests/fixtures/draft/standalone.md")],
        ReportFormat::Markdown,
        true,
        &out_md,
    );
    assert_eq!(rc, 0);
    let got = std::fs::read(&out_md).expect("markdown report file must exist");
    let want = std::fs::read(fixture("tests/fixtures/expected/draft-standalone.md")).unwrap();
    assert_eq!(got, want, "单文件 -o markdown 写盘与基线不一致");
}

/// 目录输入 `-o` 写盘：目录内按 stem 逐稿一份，内容与基线一致。
#[test]
fn draft_dir_write_golden() {
    let tmp = tempfile::tempdir().unwrap();
    for (format, ext) in [(ReportFormat::Text, "txt"), (ReportFormat::Markdown, "md")] {
        let out_dir = tmp.path().join(format!("out-{ext}"));
        let rc = run_write(
            vec![rel("tests/fixtures/draft/drafts")],
            format,
            false,
            &out_dir,
        );
        assert_eq!(rc, 0, "{ext} 目录写盘运行应成功");
        for name in ["0001-信号", "0002-断桥"] {
            let got = std::fs::read(out_dir.join(format!("{name}.{ext}")))
                .unwrap_or_else(|e| panic!("缺少 {name}.{ext}: {e}"));
            let want_path = format!("tests/fixtures/expected/draft-corpus/{name}.{ext}");
            let want = std::fs::read(fixture(want_path.as_str())).unwrap();
            assert_eq!(got, want, "{name}.{ext} 目录写盘与基线不一致");
        }
    }
}
