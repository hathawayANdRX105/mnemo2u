//! `reports-workspace` + `tools-apply` 集成测试：与 golden 基线
//! （`tests/fixtures/expected/reports-workspace-apply/*`）逐字节对照。
//!
//! 基线轮次约定：
//! - workspace 轮：先建 fixture 的 consistency db，再跑 `reports-workspace`；
//!   输入用相对路径 `tests/fixtures/...`，生成树内嵌该前缀，测试侧把标记
//!   `tests/fixtures` 替换为临时根（与 reports-catalog/backlog/kit 测试同一约定）。
//! - apply 轮：`NOVEL_REVIEW_RULES_YAML` 环境变量指向 tempdir 副本，使 `--apply`
//!   只写副本；仓内 `configs/rules/review.yaml` 全程零触碰。
//!
//! `research/consistency/*.sqlite3` 是构建产物（FTS5 影子表编码随 SQLite
//! 实现漂移，consistency 测试已用 dump+查询基线验证语义等价），故 tree
//! 基线不含 `research/`；测试侧「先 build db」步骤对齐基线生成流程。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use novel_review::config;
use novel_review::consistency;
use novel_review::reports::workspace::{self, WorkspaceOptions};
use novel_review::tools::apply::{self, ApplyOptions};

/// 串行化 `NOVEL_REVIEW_RULES_YAML` 切换（`std::env` 是进程级全局）。
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 把 `rust/` 钉为进程 cwd（与 sibling 测试同一约定；输出字节不依赖 cwd，
/// 仅与基线生成环境对齐）。
fn pin_cwd() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected_dir(variant: &str) -> PathBuf {
    fixtures_root()
        .join("expected/reports-workspace-apply")
        .join(variant)
}

/// 期望侧文件字节：把入库时嵌入的 `tests/fixtures` 标记还原为实际临时根。
fn expected_bytes(variant: &str, relative: &str, substitute: &str) -> Vec<u8> {
    let raw = fs::read(expected_dir(variant).join(relative))
        .unwrap_or_else(|err| panic!("读取期望文件失败: {variant}/{relative} ({err})"));
    String::from_utf8(raw)
        .unwrap()
        .replace("tests/fixtures", substitute)
        .into_bytes()
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

/// 建临时输入树：`<temp>/{consistency,catalog-cbk,apply-inputs}`。
fn setup_temp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    for sub in ["consistency", "catalog-cbk", "apply-inputs"] {
        copy_dir(&fixtures_root().join(sub), &root.join(sub));
    }
    (tmp, root)
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

/// 期望 tree 全部文件与临时 novel 树逐字节对照（`research/` db 不入基线）。
fn compare_tree(variant: &str, novel: &Path, temp_root: &Path) {
    let tree = expected_dir(variant).join("tree");
    let mut files = Vec::new();
    let mut stack = vec![tree.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p.strip_prefix(&tree).unwrap().display().to_string());
            }
        }
    }
    files.sort();
    for relative in files {
        let got = fs::read(novel.join(&relative))
            .unwrap_or_else(|err| panic!("round {variant}: 缺少生成文件 {relative} ({err})"));
        let want = expected_bytes(
            variant,
            &format!("tree/{relative}"),
            &temp_root.display().to_string(),
        );
        assert_eq!(got, want, "round {variant}: 文件字节不一致: {relative}");
    }
}

/// 一轮 workspace：临时树先 `build_index` 建 db，再 `workspace::run`，
/// stdout + 生成树对照基线。
fn check_workspace_round(
    variant: &str,
    fixture_rel: &str,
    sample_limit: usize,
    window_sizes: &[usize],
    temp_root: &Path,
) {
    pin_cwd();
    let novel = temp_root.join(fixture_rel);
    let db = novel.join("research/consistency/consistency.sqlite3");
    consistency::build_index(&novel, &db, &config::default_paths())
        .unwrap_or_else(|e| panic!("round {variant}: consistency build_index 失败 ({e})"));
    let opts = WorkspaceOptions {
        novel_dir: novel.clone(),
        sample_limit,
        window_sizes: window_sizes.to_vec(),
        rules: None,
        output_root: None,
    };
    let (rc, printed) =
        workspace::run(&opts).unwrap_or_else(|e| panic!("round {variant}: workspace::run ({e})"));
    assert_eq!(rc, 0, "round {variant}: 退出码应为 0");
    let want_stdout = expected_bytes(variant, "stdout.txt", &temp_root.display().to_string());
    assert_eq!(
        stdout_text(&printed),
        String::from_utf8(want_stdout).unwrap(),
        "round {variant}: stdout 不一致"
    );
    compare_tree(variant, &novel, temp_root);
}

/// 一轮 apply：dry-run stdout 与基线逐字节对照。
fn check_apply_dry_round(variant: &str, catalog: &str, temp_root: &Path) {
    pin_cwd();
    let yaml = temp_root.join("rules-yaml.yaml");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/rules/review.yaml"),
        &yaml,
    )
    .unwrap();
    let _guard = ENV_LOCK.lock();
    std::env::set_var("NOVEL_REVIEW_RULES_YAML", &yaml);
    let opts = ApplyOptions {
        catalog: temp_root.join("apply-inputs").join(catalog),
        dry_run: true,
        apply: false,
    };
    let (rc, text) = apply::run(&opts).unwrap_or_else(|e| panic!("round {variant}: ({e})"));
    assert_eq!(rc, 0, "round {variant}: 退出码应为 0");
    // 基线 out-dry.txt 是 CLI stdout（含 `__main__`/`println!` 追加的末尾换行）；
    // 库级 `run()` 返回的文本不含该换行，逐字对照前去掉恰好一个尾部 `\n`。
    let mut want = expected_bytes(variant, "out-dry.txt", &temp_root.display().to_string());
    if want.last() == Some(&b'\n') {
        want.pop();
    }
    assert_eq!(
        text,
        Some(String::from_utf8(want).unwrap()),
        "round {variant}: dry-run stdout 不一致"
    );
}

/// apply 回写轮：tempdir 副本 yaml 回写后与基线 `post-apply.yaml` 逐字节对照。
/// apply 模式的过程行已就地 `println!`（CLI 级对照已入库 `out-apply.txt`，
/// 供阶段 E golden 总验证使用；库测试仅断言回写后的 yaml 字节）。
fn check_apply_writeback_round(variant: &str, catalog: &str, temp_root: &Path) {
    pin_cwd();
    let yaml = temp_root.join("rules-yaml.yaml");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/rules/review.yaml"),
        &yaml,
    )
    .unwrap();
    let _guard = ENV_LOCK.lock();
    std::env::set_var("NOVEL_REVIEW_RULES_YAML", &yaml);
    let opts = ApplyOptions {
        catalog: temp_root.join("apply-inputs").join(catalog),
        dry_run: false,
        apply: true,
    };
    let (rc, text) = apply::run(&opts).unwrap_or_else(|e| panic!("round {variant}: ({e})"));
    assert_eq!(rc, 0, "round {variant}: 退出码应为 0");
    assert!(text.is_none(), "apply 模式不返回 dry-run 文本");
    let want = expected_bytes(variant, "post-apply.yaml", &temp_root.display().to_string());
    let got = fs::read(&yaml).unwrap();
    assert_eq!(got, want, "round {variant}: 回写后 yaml 字节不一致");
}

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 三类非法 catalog + 缺 flag：rc=1 且无 dry-run 文本（stderr 文案
/// 逐字对齐已在对照轮人工 cmp 验证，含 E2 引擎差异例外见 PORTING_NOTES）。
#[test]
fn apply_error_rounds() {
    pin_cwd();
    let _guard = env_lock();
    let inputs = fixtures_root().join("apply-inputs");
    let mk = |catalog: &str, dry_run: bool, apply: bool| ApplyOptions {
        catalog: inputs.join(catalog),
        dry_run,
        apply,
    };
    for opts in [
        // E1 Catalog not found
        mk("ghost.json", true, false),
        // E2 Invalid catalog（yaml 解析失败；前缀逐字，引擎细节见 PORTING_NOTES）
        mk("e2.yaml", true, false),
        // E3 writeback_queue 非 list / E3b 缺键
        mk("e3.yaml", true, false),
        mk("e3b.yaml", true, false),
        // E4 未指定 --dry-run/--apply
        mk("c1.yaml", false, false),
    ] {
        let (rc, text) = apply::run(&opts).unwrap();
        assert_eq!(rc, 1, "catalog {:?} 应退出码 1", opts.catalog);
        assert!(text.is_none(), "错误路径不应有 dry-run 文本");
    }
}

#[test]
fn workspace_round_default_novel1() {
    let (_tmp, root) = setup_temp();
    check_workspace_round("ws-default-novel1", "consistency/novel1", 3, &[2, 3], &root);
}

#[test]
fn workspace_round_sl0_ws2_novel1() {
    let (_tmp, root) = setup_temp();
    check_workspace_round("ws-sl0-ws2-novel1", "consistency/novel1", 0, &[2], &root);
}

#[test]
fn workspace_round_default_novela() {
    let (_tmp, root) = setup_temp();
    check_workspace_round(
        "ws-default-novela",
        "catalog-cbk/novel-a",
        3,
        &[2, 3],
        &root,
    );
}

#[test]
fn apply_round_dry_full() {
    let (_tmp, root) = setup_temp();
    check_apply_dry_round("apply-full", "c1.yaml", &root);
}

#[test]
fn apply_round_dry_empty() {
    let (_tmp, root) = setup_temp();
    check_apply_dry_round("apply-empty", "c2.json", &root);
}

#[test]
fn apply_round_writeback_full() {
    let (_tmp, root) = setup_temp();
    check_apply_writeback_round("apply-full", "c1.yaml", &root);
}
