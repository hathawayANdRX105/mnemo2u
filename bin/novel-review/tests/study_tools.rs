//! `study` 工具行为测试：`study-compare` 与 `study-pov`。
//!
//! 基线为按代码逻辑手工推导（`build_pov_candidates` 逻辑 + 密度三字段恒为 0 的说明），
//! 非运行时输出。
//!
//! `study-compare` 逻辑简单（JSON 读 + Markdown 表生成），
//! 基线同样按代码逻辑手工推导。

use novel_review::study::compare;
use novel_review::study::pov;

// ── study-compare 库级测试 ──────────────────────────────────────────────────

/// 表头 + 分隔行 + 一行数字指标：
/// 1 表头 + 1 分隔 + 1 数字行 + 1 非数字行 = 4 行（不含表头/分隔前的内容）。
#[test]
fn compare_table_structure() {
    let baseline = serde_json::json!({"summary": {"a": 1, "b": "foo"}});
    let applied = serde_json::json!({"summary": {"a": 2, "b": "bar"}});
    let table = compare::build_compare_table(&baseline, &applied);
    let lines: Vec<&str> = table.lines().collect();

    // 表头 + 分隔 + "a" 数字行 + "b" 非数字行 = 4 行
    assert_eq!(lines.len(), 4, "应为 4 行: {table}");
    assert_eq!(
        lines[0], "| Metric | Baseline | Applied | Delta |",
        "表头不对"
    );
    assert!(
        lines[1].starts_with("|----"),
        "第二行应为分隔行: {}",
        lines[1]
    );
    // "a" 行：int-int → int delta（不带 .0）
    assert!(
        lines[2].contains("| a | 1 | 2 | 1 |"),
        "数字行应为 int 渲染: {}",
        lines[2]
    );
    // "b" 行：非数字 → 不参与比较
    assert!(
        lines[3].contains("不参与比较"),
        "非数字行应含'不参与比较': {}",
        lines[3]
    );
}

/// `audit-draft --format json` 的顶层数组输出可直接喂给 study-compare
/// （本模块对同样输入取首元素出表）。空数组报错。
#[test]
fn compare_accepts_audit_json_array_report() {
    let tmp = tempfile::tempdir().unwrap();
    let baseline = tmp.path().join("a.json");
    let applied = tmp.path().join("b.json");
    std::fs::write(
        &baseline,
        serde_json::to_vec(&serde_json::json!([{"summary": {"chars": 100}}])).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &applied,
        serde_json::to_vec(&serde_json::json!([{"summary": {"chars": 250}}])).unwrap(),
    )
    .unwrap();

    let table = compare::run_compare(&baseline, &applied).unwrap();
    assert!(
        table.contains("| chars | 100 | 250 | 150 |"),
        "数组报告应产出 chars 指标行: {table}"
    );

    let empty = tmp.path().join("empty.json");
    std::fs::write(&empty, "[]").unwrap();
    let err = compare::run_compare(&empty, &applied).unwrap_err();
    assert!(err.to_string().contains("空数组"), "空数组应报错: {err}");
}

/// schema_version 不等 → 错误消息含双方版本号。
#[test]
fn compare_schema_mismatch_error() {
    let baseline = serde_json::json!({"schema_version": 1, "summary": {"a": 1}});
    let applied = serde_json::json!({"schema_version": 2, "summary": {"a": 2}});

    // run_compare 需要文件路径；这里直接构造 Value 无法走 run_compare，
    // 改用临时文件测试。
    let tmp = tempfile::tempdir().unwrap();
    let bp = tmp.path().join("baseline.json");
    let ap = tmp.path().join("applied.json");
    std::fs::write(&bp, serde_json::to_string(&baseline).unwrap()).unwrap();
    std::fs::write(&ap, serde_json::to_string(&applied).unwrap()).unwrap();

    let result = compare::run_compare(&bp, &ap);
    let err = result.expect_err("schema_version 不等应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("schema_version mismatch"),
        "错误消息应含 'schema_version mismatch': {msg}"
    );
    assert!(msg.contains("1"), "错误消息应含基线版本号: {msg}");
    assert!(msg.contains("2"), "错误消息应含应用版本号: {msg}");
}

/// 二级 dict 内数字指标提取（`k.kk` 键名）。
#[test]
fn compare_nested_numeric_metrics() {
    let baseline = serde_json::json!({"summary": {"sub": {"x": 10, "y": 20.5}}});
    let applied = serde_json::json!({"summary": {"sub": {"x": 15, "y": 25.5}}});
    let table = compare::build_compare_table(&baseline, &applied);

    // 两行数字指标：sub.x（int）+ sub.y（float）
    assert!(
        table.contains("| sub.x | 10 | 15 | 5 |"),
        "int 二级指标应正确渲染: {table}"
    );
    assert!(
        table.contains("| sub.y | 20.5 | 25.5 | 5.0 |"),
        "float 二级指标应正确渲染: {table}"
    );
}

/// float 差值渲染：int - int → int（无 `.0`）；float - int → float（`.0`）。
#[test]
fn compare_float_delta_rendering() {
    let baseline = serde_json::json!({"summary": {"f1": 10.0, "f2": 10.5}});
    let applied = serde_json::json!({"summary": {"f1": 15.0, "f2": 11.0}});
    let table = compare::build_compare_table(&baseline, &applied);

    // f1: 15.0 - 10.0 = 5.0（float 运算 → `float_repr` → "5.0"）
    assert!(
        table.contains("| f1 | 10.0 | 15.0 | 5.0 |"),
        "float 差值应渲染 5.0: {table}"
    );
    // f2: 11.0 - 10.5 = 0.5（float 运算 → `float_repr` → "0.5"）
    assert!(
        table.contains("| f2 | 10.5 | 11.0 | 0.5 |"),
        "float 差值 0.5 应正确渲染: {table}"
    );
}

/// 缺 summary 键 → 容忍（空表，只有表头+分隔行）。
#[test]
fn compare_missing_summary_tolerance() {
    let baseline = serde_json::json!({"other": 1});
    let applied = serde_json::json!({"other": 2});
    let table = compare::build_compare_table(&baseline, &applied);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 2, "缺 summary 应只有表头+分隔行: {table}");
}

// ── study-pov 库级测试 ─────────────────────────────────────────────────────

/// pov 确定性：同一输入两次调用 → 字节相等。
#[test]
fn pov_deterministic() {
    let text = "# Chapter 1\n\n我看着窗外。\n\n他走了进来。\n\n她说：“你好。”\n\n他们笑了。\n";
    let splitter = novel_review::text::TextSplitter::new(
        r"^\s*(?:#|\||[-*]\s*(?:状态|所属分类|卡片 ID|别名|首次生效|当前位置|时间线状态|关联卡片|看点|状态变化|伏笔动作|节奏|本章功能|章节钩子|推荐章数|定位|核心冲突|核心事件|章数|Scene|字段|章节|目的)[:：])",
    )
    .unwrap();

    let a1 = serde_json::to_string_pretty(&pov::build_pov_candidates(text, &splitter)).unwrap();
    let a2 = serde_json::to_string_pretty(&pov::build_pov_candidates(text, &splitter)).unwrap();
    assert_eq!(a1, a2, "pov 输出应确定性");
}

/// pov schema_version 与 `audit-draft --format json` 输出一致（当前取 1）。
#[test]
fn pov_schema_version_is_one() {
    let text = "一段中文正文。\n";
    let splitter = novel_review::text::TextSplitter::new(r"^\s*#").unwrap();
    let analysis = pov::build_pov_candidates(text, &splitter);
    assert_eq!(
        serde_json::to_value(&analysis).unwrap()["schema_version"],
        serde_json::json!(1),
        "schema_version 应为 1"
    );
}

/// 给定样例文本（标题 + 四段中文）的候选结构基线。
///
/// 按代码逻辑手工推导。注意：`split_paragraph_infos` 不过滤 `#` 标题行（
/// `markdown_noise` 只在 `split_sentence_infos` 里生效），
/// 所以 `# Chapter 1` 计为第 1 段，共 5 段。
/// - 5 段：`line_start` 分别为 1, 3, 5, 7, 9。
/// - 所有密度 = 0（pronouns/personal_names 永远为空）。
/// - 候选 = 1 个尾组：`paragraph_range="0-5"`，`evidence_line_numbers=[1,3,5,7,9]`。
#[test]
fn pov_candidate_structure_for_sample() {
    let text = "# Chapter 1\n\n我看着窗外。\n\n他走了进来。\n\n她说：“你好。”\n\n他们笑了。\n";
    let splitter = novel_review::text::TextSplitter::new(
        r"^\s*(?:#|\||[-*]\s*(?:状态|所属分类|卡片 ID|别名|首次生效|当前位置|时间线状态|关联卡片|看点|状态变化|伏笔动作|节奏|本章功能|章节钩子|推荐章数|定位|核心冲突|核心事件|章数|Scene|字段|章节|目的)[:：])",
    )
    .unwrap();

    let analysis = pov::build_pov_candidates(text, &splitter);
    let json = serde_json::to_value(&analysis).unwrap();

    // 5 段（`# Chapter 1` 不过滤，计为第 1 段）
    assert_eq!(
        json["chapter_summary"]["total_paragraphs"], 5,
        "共 5 段（标题行不过滤）: {json}"
    );

    let candidates = &json["chapter_summary"]["candidates"];
    assert_eq!(
        candidates.as_array().unwrap().len(),
        1,
        "密度恒 0 时只有尾组 1 个候选: {json}"
    );

    let c0 = &candidates[0];
    assert_eq!(c0["candidate"], "POV shift candidate");
    assert_eq!(c0["paragraph_range"], "0-5");
    // 5 段起始行：1, 3, 5, 7, 9
    assert_eq!(
        c0["evidence_line_numbers"],
        serde_json::json!([1, 3, 5, 7, 9]),
        "证据行号应为 [1,3,5,7,9]: {json}"
    );
    // 密度全 0：`(count, density)` 元组 → JSON 数组 `[0, 0.0]`
    assert_eq!(c0["pronoun_density"], serde_json::json!([0, 0.0]));
    assert_eq!(c0["personal_name_density"], serde_json::json!([0, 0.0]));
    assert_eq!(c0["dialogue_attribution"], serde_json::json!([0, 0.0]));
}

/// 空文本 → 0 段，无候选。
#[test]
fn pov_empty_text_no_candidates() {
    let splitter = novel_review::text::TextSplitter::new(r"^\s*#").unwrap();
    let analysis = pov::build_pov_candidates("", &splitter);
    let json = serde_json::to_value(&analysis).unwrap();
    assert_eq!(json["chapter_summary"]["total_paragraphs"], 0);
    assert!(
        json["chapter_summary"]["candidates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "空文本无候选: {json}"
    );
}

// ── CLI 端到端测试 ────────────────────────────────────────────────────────

/// 运行 `novel-review` 二进制的 `study-compare` / `study-pov` 子命令。
fn run_study_cli(args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_novel-review"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("spawn novel-review binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status.code().unwrap_or(-1), stdout, stderr)
}

/// CLI `study-compare`：schema mismatch → rc=1 + stderr 含 "Error: schema_version mismatch"。
#[test]
fn cli_study_compare_schema_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let bp = tmp.path().join("b.json");
    let ap = tmp.path().join("a.json");
    std::fs::write(&bp, r#"{"schema_version":1,"summary":{"a":1}}"#).unwrap();
    std::fs::write(&ap, r#"{"schema_version":2,"summary":{"a":2}}"#).unwrap();

    let (rc, _stdout, stderr) =
        run_study_cli(&["study-compare", bp.to_str().unwrap(), ap.to_str().unwrap()]);
    assert_eq!(rc, 1, "schema mismatch 应 rc=1: stderr={stderr}");
    assert!(
        stderr.contains("schema_version mismatch"),
        "stderr 应含 'schema_version mismatch': {stderr}"
    );
    assert!(
        stderr.contains("Error:"),
        "stderr 应以 'Error:' 开头: {stderr}"
    );
}

/// CLI `study-compare`：正常对比 → rc=0 + stdout 含 Markdown 表。
#[test]
fn cli_study_compare_success() {
    let tmp = tempfile::tempdir().unwrap();
    let bp = tmp.path().join("b.json");
    let ap = tmp.path().join("a.json");
    std::fs::write(
        &bp,
        r#"{"schema_version":1,"summary":{"x":10,"y":"ignored"}}"#,
    )
    .unwrap();
    std::fs::write(
        &ap,
        r#"{"schema_version":1,"summary":{"x":15,"y":"ignored"}}"#,
    )
    .unwrap();

    let (rc, stdout, stderr) =
        run_study_cli(&["study-compare", bp.to_str().unwrap(), ap.to_str().unwrap()]);
    assert_eq!(rc, 0, "正常对比应 rc=0: stderr={stderr}");
    assert!(
        stdout.contains("| Metric | Baseline | Applied | Delta |"),
        "stdout 应含表头: {stdout}"
    );
    assert!(
        stdout.contains("| x | 10 | 15 | 5 |"),
        "stdout 应含 x 行: {stdout}"
    );
    assert!(
        stdout.contains("| y | 不参与比较 | 不参与比较 | 不参与比较 |"),
        "stdout 应含 y 非数字行: {stdout}"
    );
}

/// CLI `study-pov`：给定章节文件 → rc=0 + stdout 为 JSON 且含 schema_version=1。
#[test]
fn cli_study_pov_success() {
    let tmp = tempfile::tempdir().unwrap();
    let ch = tmp.path().join("ch1.md");
    std::fs::write(&ch, "# 第一章\n\n我看着窗外。\n\n他走了进来。\n").unwrap();

    let (rc, stdout, stderr) = run_study_cli(&["study-pov", ch.to_str().unwrap()]);
    assert_eq!(rc, 0, "study-pov 应 rc=0: stderr={stderr}");

    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("stdout 应为合法 JSON");
    assert_eq!(parsed["schema_version"], serde_json::json!(1));
    assert_eq!(parsed["heuristic"], serde_json::json!(true));
    assert_eq!(parsed["confidence"], serde_json::json!("low"));
}

/// CLI `study-pov`：确定性（两次调用字节相等）。
#[test]
fn cli_study_pov_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let ch = tmp.path().join("ch1.md");
    std::fs::write(&ch, "# 第一章\n\n我看着窗外。\n\n他走了进来。\n").unwrap();

    let args: Vec<&str> = vec!["study-pov", ch.to_str().unwrap()];
    let (_rc1, out1, _) = run_study_cli(&args);
    let (_rc2, out2, _) = run_study_cli(&args);
    assert_eq!(out1, out2, "两次 study-pov 调用 stdout 应字节相等");
}
