//! consistency 模块字节级对照测试：基线在 `tests/fixtures/consistency/expected/`。
//! 比对前将 fixture novel 目录的绝对前缀归一为 `{{NOVEL}}`（feedback-add 的
//! stdout/JSONL 路径为绝对路径，其余子命令输出均为相对路径）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const NOVEL: &str = "tests/fixtures/consistency/novel1";
const DB_REL: &str = "research/consistency/consistency.sqlite3";
const EXP: &str = "tests/fixtures/consistency/expected";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_novel-review"))
}

fn stdout_of(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn novel-review binary {args:?}: {e}"))
}

fn exp(name: &str) -> String {
    fs::read_to_string(format!("{EXP}/{name}"))
        .unwrap_or_else(|e| panic!("missing baseline {EXP}/{name}: {e}"))
}

/// 将 novel fixture 的绝对前缀归一为 `{{NOVEL}}`，使两侧可比。
fn norm(s: &str, novel_abs: &str) -> String {
    s.replace(novel_abs, "{{NOVEL}}")
}

#[test]
fn build_queries_and_feedback_parity() {
    let novel_abs = Path::new(NOVEL)
        .canonicalize()
        .unwrap_or_else(|e| panic!("canonicalize {NOVEL}: {e}"))
        .to_string_lossy()
        .into_owned();

    let db = Path::new(NOVEL).join(DB_REL);
    let jsonl = Path::new(NOVEL).join("research/consistency/review-feedback.jsonl");
    let _ = fs::remove_file(&db);
    let _ = fs::remove_file(&jsonl);

    // 1) build：stdout 与 golden 基线逐字节一致。
    let o = run(&["consistency", "build", NOVEL]);
    assert!(o.status.success(), "build failed: {o:?}");
    assert_eq!(stdout_of(&o), exp("build.txt"), "build stdout");

    // 2) 查询子命令：逐字节 stdout 基线（d.path 为相对路径）。
    for (cmd, extra, baseline) in [
        ("search", &["信号枪"] as &[&str], "search.txt"),
        ("entity", &["陆沉"], "entity.txt"),
        ("facts", &["陆沉"], "facts.txt"),
        ("story-facts", &["story1"], "story_facts.txt"),
        ("tension", &[] as &[&str], "tension.txt"),
        ("conflicts", &[], "conflicts.txt"),
        ("catalog", &[], "catalog.txt"),
        ("suspects", &[], "suspects.txt"),
        ("alignment", &[], "alignment.txt"),
    ] {
        let mut args = vec!["consistency", cmd, NOVEL];
        args.extend_from_slice(extra);
        let args: Vec<&str> = args;
        let o = run(&args);
        assert!(o.status.success(), "{cmd} failed: {o:?}");
        assert_eq!(
            norm(&stdout_of(&o), &novel_abs),
            norm(&exp(baseline), &novel_abs),
            "{cmd} stdout"
        );
    }

    // 3) feedback-add：stdout（反馈路径归一）+ JSONL 内容（updated_at 除外）。
    let o = run(&[
        "consistency",
        "feedback-add",
        NOVEL,
        "--category",
        "equipment_state_jump",
        "--story",
        "story1",
        "--title",
        "信号枪",
        "--decision",
        "watch",
        "--facet",
        "state_progression",
        "--note",
        "复核",
    ]);
    assert!(o.status.success(), "feedback-add failed: {o:?}");
    assert_eq!(
        norm(&stdout_of(&o), &novel_abs),
        norm(&exp("feedback-add.txt"), &novel_abs),
        "feedback-add stdout"
    );
    let jsonl_text =
        fs::read_to_string(&jsonl).unwrap_or_else(|e| panic!("missing {jsonl:?}: {e}"));
    let lines: Vec<&str> = jsonl_text.lines().collect();
    assert_eq!(lines.len(), 1, "feedback JSONL 行数");
    let got: Value = serde_json::from_str(lines[0])
        .unwrap_or_else(|e| panic!("feedback JSONL 解析失败 {}: {e}", lines[0]));
    let want: Value = serde_json::from_str(exp("feedback-add.jsonl.txt").trim())
        .unwrap_or_else(|e| panic!("baseline JSONL 解析失败: {e}"));
    for (k, v) in want.as_object().expect("baseline JSONL 对象") {
        if k == "updated_at" {
            continue;
        }
        assert_eq!(got.get(k), Some(v), "feedback JSONL 字段 {k}");
    }
    // updated_at：Rust 侧写真实 RFC3339 时间戳，仅校验格式。
    let ts = got
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or("missing updated_at");
    chrono::DateTime::parse_from_rfc3339(ts)
        .unwrap_or_else(|e| panic!("updated_at 非 RFC3339: {ts}: {e}"));

    // 4) 反馈后查询：feedback-summary / review-queue / suspects。
    for (cmd, baseline) in [
        ("feedback-summary", "feedback-summary.txt"),
        ("review-queue", "review-queue.txt"),
        ("suspects", "suspects-after.txt"),
    ] {
        let o = run(&["consistency", cmd, NOVEL]);
        assert!(o.status.success(), "{cmd} failed: {o:?}");
        assert_eq!(
            norm(&stdout_of(&o), &novel_abs),
            norm(&exp(baseline), &novel_abs),
            "{cmd} stdout"
        );
    }
}
