//! `reports-catalog` / `reports-backlog` / `reports-kit` 集成测试：与 golden 基线
//! （`tests/fixtures/expected/reports-{catalog,backlog,kit}/*`）逐字节对照。
//!
//! 基线以相对输入路径（`tests/fixtures/catalog-cbk/...`）定稿。报告内嵌输入路径
//! （`- source:` 与 stdout 行、`CANDIDATES.json` 的 `story` 字段、review-kit 章节
//! 路径），故测试侧把 `tests/fixtures/catalog-cbk` 拷到临时根目录运行，按
//! `tests/fixtures` → 临时根的标记替换再逐字节对照。
//!
//! 轮次：catalog ×4（`c-full` / `c-novelb` / `c-single` / `c-stats`，其中
//! `c-stats` 先跑 backlog 再让 catalog 走 `draft-stats` 读取支），
//! backlog ×4（默认 / `--sample-limit 0` / `50` / novel-b），
//! kit ×4（默认 / 0 / 50 / novel-b）。

use std::fs;
use std::path::{Path, PathBuf};

use novel_review::reports::backlog::{run as run_backlog, BacklogOptions};
use novel_review::reports::catalog::{run as run_catalog, CatalogOptions};
use novel_review::reports::kit::{run as run_kit, KitOptions};

/// 把 `rust/` 钉为进程 cwd（与 stats/scorecard 测试同一约定；对照输入为
/// 临时根目录下的绝对路径，cwd 本身不影响输出字节，仅与基线生成环境对齐）。
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

/// 建临时输入树：`<temp>/catalog-cbk`。
fn setup_temp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    copy_dir(
        &fixtures_root().join("catalog-cbk"),
        &root.join("catalog-cbk"),
    );
    (tmp, root)
}

/// 期望侧文件字节：把入库时嵌入的 `tests/fixtures` 标记还原为实际临时根。
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

/// 期望目录全部文件（除 stdout 快照）与临时树逐字节对照。
fn compare_tree(module: &str, variant: &str, temp_root: &Path) {
    let files = list_files(&expected_dir(module, variant))
        .into_iter()
        .filter(|f| f != "stdout.txt" && f != "backlog-stdout.txt")
        .collect::<Vec<_>>();
    for relative in &files {
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

/// stdout 逐字节对照。
fn compare_stdout(module: &str, variant: &str, snapshot: &str, printed: &str, temp_root: &Path) {
    let want = expected_bytes(module, variant, snapshot, temp_root);
    let want_str = String::from_utf8(want).unwrap();
    assert_eq!(
        printed, want_str,
        "round {module}/{variant}: stdout 不一致 ({snapshot})"
    );
}

/// `run` 返回的展示路径列表还原为 CLI stdout（与 `main.rs` 逐行
/// `println!("{}", path.display())` 等价）。
fn stdout_text(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// 一轮 catalog：临时树跑 `run_catalog`，stdout + 生成树对照。
fn check_catalog_round(variant: &str, input: &Path, temp_root: &Path) {
    let opts = CatalogOptions {
        paths: vec![input.to_path_buf()],
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run_catalog(&opts).unwrap();
    assert_eq!(rc, 0, "catalog {variant}: rc 应为 0");
    compare_stdout(
        "catalog",
        variant,
        "stdout.txt",
        &stdout_text(&printed),
        temp_root,
    );
    compare_tree("catalog", variant, temp_root);
}

/// 一轮 backlog。
fn check_backlog_round(variant: &str, input: &Path, sample_limit: usize, temp_root: &Path) {
    let opts = BacklogOptions {
        paths: vec![input.to_path_buf()],
        sample_limit,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run_backlog(&opts).unwrap();
    assert_eq!(rc, 0, "backlog {variant}: rc 应为 0");
    compare_stdout(
        "backlog",
        variant,
        "stdout.txt",
        &stdout_text(&printed),
        temp_root,
    );
    compare_tree("backlog", variant, temp_root);
}

/// 一轮 kit。
fn check_kit_round(variant: &str, input: &Path, sample_limit: usize, temp_root: &Path) {
    let opts = KitOptions {
        paths: vec![input.to_path_buf()],
        sample_limit,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run_kit(&opts).unwrap();
    assert_eq!(rc, 0, "kit {variant}: rc 应为 0");
    compare_stdout(
        "kit",
        variant,
        "stdout.txt",
        &stdout_text(&printed),
        temp_root,
    );
    compare_tree("kit", variant, temp_root);
}

#[test]
fn catalog_round_c_full() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_catalog_round("c-full", &root.join("catalog-cbk/novel-a"), &root);
}

#[test]
fn catalog_round_c_novelb() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_catalog_round("c-novelb", &root.join("catalog-cbk/novel-b"), &root);
}

#[test]
fn catalog_round_c_single() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_catalog_round(
        "c-single",
        &root.join("catalog-cbk/novel-a/drafts/arc1/story1/ch01-信号.md"),
        &root,
    );
}

#[test]
fn catalog_round_c_stats() {
    // 基线生成序：先 backlog（默认 sample_limit=6）写 draft-stats，catalog 走读取支。
    pin_cwd();
    let (_tmp, root) = setup_temp();
    let novel = root.join("catalog-cbk/novel-a");
    let backlog_opts = BacklogOptions {
        paths: vec![novel.clone()],
        sample_limit: 6,
        rules: None,
        output_root: None,
    };
    let (rc, printed) = run_backlog(&backlog_opts).unwrap();
    assert_eq!(rc, 0, "c-stats 前置 backlog: rc 应为 0");
    compare_stdout(
        "catalog",
        "c-stats",
        "backlog-stdout.txt",
        &stdout_text(&printed),
        &root,
    );
    check_catalog_round("c-stats", &novel, &root);
}

#[test]
fn backlog_round_b_default() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_backlog_round("b-default", &root.join("catalog-cbk/novel-a"), 6, &root);
}

#[test]
fn backlog_round_b_sl0() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_backlog_round("b-sl0", &root.join("catalog-cbk/novel-a"), 0, &root);
}

#[test]
fn backlog_round_b_sl50() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_backlog_round("b-sl50", &root.join("catalog-cbk/novel-a"), 50, &root);
}

#[test]
fn backlog_round_b_novelb() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_backlog_round("b-novelb", &root.join("catalog-cbk/novel-b"), 6, &root);
}

#[test]
fn kit_round_k_default() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_kit_round("k-default", &root.join("catalog-cbk/novel-a"), 6, &root);
}

#[test]
fn kit_round_k_sl0() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_kit_round("k-sl0", &root.join("catalog-cbk/novel-a"), 0, &root);
}

#[test]
fn kit_round_k_sl50() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_kit_round("k-sl50", &root.join("catalog-cbk/novel-a"), 50, &root);
}

#[test]
fn kit_round_k_novelb() {
    pin_cwd();
    let (_tmp, root) = setup_temp();
    check_kit_round("k-novelb", &root.join("catalog-cbk/novel-b"), 6, &root);
}
