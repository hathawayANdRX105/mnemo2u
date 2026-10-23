//! `reports-learning` / `reports-profiles` 集成测试：与 golden 基线逐字节对照
//! （`tests/fixtures/expected/reports-{learning,profiles}/{轮次}`）。
//!
//! 基线以相对输入路径（`tests/fixtures/...`、同 cwd）定稿；
//! 报告内嵌输入路径（`- source: ...` 与 stdout 行、db 轮 `feedback_log` 绝对路径），
//! 故测试侧把输入拷到临时根目录运行后，按 `tests/fixtures` → 临时根的标记替换再逐字节对照。
//! `--output-root` 隐藏参数轮（`output_root`）在测试侧经 API 对照生成树；
//! 其 stdout 与 CLI 文案由 `reports_learning_profiles_cli_round` 守护。

use std::fs;
use std::path::{Path, PathBuf};

use novel_review::reports::learning::{run as run_learning, LearningOptions};
use novel_review::reports::profiles::{run as run_profiles, ProfileOptions};

/// 把 `tests/fixtures` 钉为进程 cwd（与 stats/scorecard 测试同一约定）。
fn pin_cwd() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected_dir(module: &str, variant: &str) -> PathBuf {
    fixtures_root()
        .join("expected")
        .join(format!("reports-{module}"))
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

/// 建临时输入树：`<temp>/stats-draft`（+ 需要时 `<temp>/draft`、`<temp>/consistency`）。
fn setup_temp(with_empty_fixture: bool, with_consistency: bool) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    copy_dir(
        &fixtures_root().join("stats-draft"),
        &root.join("stats-draft"),
    );
    if with_empty_fixture {
        copy_dir(&fixtures_root().join("draft"), &root.join("draft"));
    }
    if with_consistency {
        copy_dir(
            &fixtures_root().join("consistency"),
            &root.join("consistency"),
        );
    }
    (tmp, root)
}

/// 期望侧文件字节：把入库时规范化的 `tests/fixtures` 标记还原为实际临时根。
fn expected_bytes(module: &str, variant: &str, relative: &str, temp_root: &Path) -> Vec<u8> {
    let raw = fs::read(expected_dir(module, variant).join(relative))
        .unwrap_or_else(|err| panic!("读取期望文件失败: {module}/{variant}/{relative} ({err})"));
    String::from_utf8(raw)
        .unwrap()
        .replace("tests/fixtures", &temp_root.display().to_string())
        .into_bytes()
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

/// 生成树逐字节对照（`files` 为期望目录内除 `stdout.txt` 外的相对布局）。
fn compare_tree(module: &str, variant: &str, files: &[String], temp_root: &Path) {
    for relative in files {
        let got = fs::read(temp_root.join(relative)).unwrap_or_else(|err| {
            panic!("round {module}/{variant}: 缺少生成文件 {relative} ({err})")
        });
        let want = expected_bytes(module, variant, relative, temp_root);
        assert_eq!(
            got, want,
            "round {module}/{variant}: 文件字节不一致: {relative}"
        );
    }
}

/// 一轮 learning 对照：生成树（`draft-stats/learning`）+ stdout 逐字节一致。
fn check_learning_round(variant: &str, input: &Path, sample_limit: usize, temp_root: &Path) {
    let opts = LearningOptions {
        paths: vec![input.to_path_buf()],
        sample_limit,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run_learning(&opts).unwrap();
    assert_eq!(rc, 0, "learning {variant}: rc 应为 0");
    let want_files = list_files(&expected_dir("learning", variant))
        .into_iter()
        .filter(|f| f != "stdout.txt")
        .collect::<Vec<_>>();
    compare_tree("learning", variant, &want_files, temp_root);

    let stdout = expected_bytes("learning", variant, "stdout.txt", temp_root);
    let got_stdout = printed
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut got_stdout = got_stdout.into_bytes();
    got_stdout.push(b'\n');
    assert_eq!(stdout, got_stdout, "learning {variant}: stdout 不一致");
}

/// 一轮 profiles 对照（缺省树）：生成树（`draft-stats/profiles`）+ stdout 逐字节一致。
fn check_profiles_round(variant: &str, input: &Path, sample_limit: usize, temp_root: &Path) {
    let opts = ProfileOptions {
        paths: vec![input.to_path_buf()],
        sample_limit,
        output_root: None,
        rules: None,
    };
    let (rc, printed) = run_profiles(&opts).unwrap();
    assert_eq!(rc, 0, "profiles {variant}: rc 应为 0");
    let want_files = list_files(&expected_dir("profiles", variant))
        .into_iter()
        .filter(|f| f != "stdout.txt")
        .collect::<Vec<_>>();
    compare_tree("profiles", variant, &want_files, temp_root);

    let stdout = expected_bytes("profiles", variant, "stdout.txt", temp_root);
    let got_stdout = printed
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut got_stdout = got_stdout.into_bytes();
    got_stdout.push(b'\n');
    assert_eq!(stdout, got_stdout, "profiles {variant}: stdout 不一致");
}

/// 默认轮：`novel-a/drafts` 全树（语料学习开启），learning 树 + SUMMARY + stdout 逐字节一致。
#[test]
fn learning_default_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_learning_round(
        "default",
        &temp.join("stats-draft/novel-a/drafts"),
        6,
        &temp,
    );
}

/// 单章轮：仅 `ch01`（单章镜像 SUMMARY）。
#[test]
fn learning_single_chapter_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_learning_round(
        "single",
        &temp.join("stats-draft/novel-a/drafts/ch01-信号.md"),
        6,
        &temp,
    );
}

/// 采样上限极值轮：`--sample-limit 0`（aa_bb 首样本受 cap 约束、样例行消失）。
#[test]
fn learning_limit0_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_learning_round("limit0", &temp.join("stats-draft/novel-a/drafts"), 0, &temp);
}

/// 采样上限高值轮：`--sample-limit 50`（不截断，覆盖全部候选）。
#[test]
fn learning_limit50_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_learning_round(
        "limit50",
        &temp.join("stats-draft/novel-a/drafts"),
        50,
        &temp,
    );
}

/// 第二小说轮：`novel-b/drafts`（不同 story 组）。
#[test]
fn learning_novelb_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_learning_round("novelb", &temp.join("stats-draft/novel-b/drafts"), 6, &temp);
}

/// 一致性 db 轮：`consistency/novel1`（SQLite 快照可用，learning 日志含反馈行）。
#[test]
fn learning_db_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, true);
    check_learning_round("db", &temp.join("consistency/novel1/drafts"), 6, &temp);
}

/// 空输入护栏：`draft/drafts`（无 `ch` 数字章节名）→ rc=1、无打印、无输出文件。
#[test]
fn learning_empty_input_guardrail() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(true, false);
    let opts = LearningOptions {
        paths: vec![temp.join("draft/drafts")],
        sample_limit: 6,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run_learning(&opts).unwrap();
    assert_eq!(rc, 1);
    assert!(printed.is_empty());
    assert!(
        !temp.join("draft/draft-stats").exists(),
        "空输入不应写出任何文件"
    );
}

/// 默认轮：`novel-a/drafts` 全树，profiles 树 + SUMMARY + stdout 逐字节一致。
#[test]
fn profiles_default_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_profiles_round(
        "default",
        &temp.join("stats-draft/novel-a/drafts"),
        8,
        &temp,
    );
}

/// 单章轮：仅 `ch01`。
#[test]
fn profiles_single_chapter_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_profiles_round(
        "single",
        &temp.join("stats-draft/novel-a/drafts/ch01-信号.md"),
        8,
        &temp,
    );
}

/// 采样上限极值轮：`--sample-limit 0`（各节 `- 无`、样例/证据行消失）。
#[test]
fn profiles_limit0_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_profiles_round("limit0", &temp.join("stats-draft/novel-a/drafts"), 0, &temp);
}

/// 采样上限高值轮：`--sample-limit 50`（不截断）。
#[test]
fn profiles_limit50_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_profiles_round(
        "limit50",
        &temp.join("stats-draft/novel-a/drafts"),
        50,
        &temp,
    );
}

/// 第二小说轮：`novel-b/drafts`。
#[test]
fn profiles_novelb_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    check_profiles_round("novelb", &temp.join("stats-draft/novel-b/drafts"), 8, &temp);
}

/// 隐藏参数轮：`--output-root`（API 侧对照 `out/` 镜像树；默认树不生成）。
#[test]
fn profiles_output_root_round_bytes_match() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(false, false);
    let out_root = temp.join("out");
    let opts = ProfileOptions {
        paths: vec![temp.join("stats-draft/novel-a/drafts")],
        sample_limit: 3,
        output_root: Some(out_root.clone()),
        rules: None,
    };
    let (rc, _printed) = run_profiles(&opts).unwrap();
    assert_eq!(rc, 0, "profiles output_root: rc 应为 0");
    let want_files = list_files(&expected_dir("profiles", "output_root"))
        .into_iter()
        .filter(|f| f != "stdout.txt")
        .collect::<Vec<_>>();
    compare_tree("profiles", "output_root", &want_files, &temp);
    assert!(
        !temp.join("stats-draft/novel-a/draft-stats").exists(),
        "指定 output-root 时不应写小说本地 draft-stats 树"
    );
}

/// 空输入护栏：`draft/drafts` → rc=1、无打印、无输出文件。
#[test]
fn profiles_empty_input_guardrail() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(true, false);
    let opts = ProfileOptions {
        paths: vec![temp.join("draft/drafts")],
        sample_limit: 8,
        output_root: None,
        rules: None,
    };
    let (rc, printed) = run_profiles(&opts).unwrap();
    assert_eq!(rc, 1);
    assert!(printed.is_empty());
    assert!(
        !temp.join("draft/draft-stats").exists(),
        "空输入不应写出任何文件"
    );
}

/// CLI 端到端轮：spawn `novel-review reports-learning` / `reports-profiles`，
/// stdout/退出码与 golden 基线一致；`--output-root` 隐藏参数轮同 cwd 对照；
/// 空输入轮 stderr 文案与退出码亦对齐。
#[test]
fn reports_learning_profiles_cli_round() {
    pin_cwd();
    let (_tmp, temp) = setup_temp(true, false);
    let bin = env!("CARGO_BIN_EXE_novel-review");

    // reports-learning 默认参数（缺省 sample-limit 6）
    let out = std::process::Command::new(bin)
        .args(["reports-learning"])
        .arg(temp.join("stats-draft/novel-a/drafts"))
        .output()
        .unwrap();
    assert!(out.status.success(), "CLI learning rc: {}", out.status);
    let want_stdout = expected_bytes("learning", "default", "stdout.txt", &temp);
    assert_eq!(out.stdout, want_stdout, "CLI learning stdout 不一致");

    // reports-profiles 默认参数（缺省 sample-limit 8）
    let out = std::process::Command::new(bin)
        .args(["reports-profiles"])
        .arg(temp.join("stats-draft/novel-a/drafts"))
        .output()
        .unwrap();
    assert!(out.status.success(), "CLI profiles rc: {}", out.status);
    let want_stdout = expected_bytes("profiles", "default", "stdout.txt", &temp);
    assert_eq!(out.stdout, want_stdout, "CLI profiles stdout 不一致");

    // 隐藏参数 --output-root：与基线生成同 cwd（rust/），生成 `out/` 后清理
    let out = std::process::Command::new(bin)
        .args(["reports-profiles"])
        .arg(temp.join("stats-draft/novel-a/drafts"))
        .args(["--output-root", "out"])
        .arg("--sample-limit")
        .arg("3")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "CLI profiles --output-root rc: {}",
        out.status
    );
    let want_stdout = expected_bytes("profiles", "output_root", "stdout.txt", &temp);
    assert_eq!(
        out.stdout, want_stdout,
        "CLI profiles --output-root stdout 不一致"
    );
    fs::remove_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("out")).ok();

    // 空输入护栏：两侧文案与退出码一致
    for sub in ["reports-learning", "reports-profiles"] {
        let out = std::process::Command::new(bin)
            .arg(sub)
            .arg(temp.join("draft/drafts"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{sub} 空输入 rc 应为 1");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("No draft chapter files found."),
            "{sub} 空输入 stderr 文案不一致: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
