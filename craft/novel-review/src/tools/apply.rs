//! `tools.apply`：模板候选回写 dry-run 预览 / apply 写回 review.yaml。
//!
//! - dry-run 文本：`render_dry_run` 逐行稳定输出。
//! - apply 模式经 serde_yaml 原样读改写 `draft` 节（键序保持），
//!   YAML dump 与 safe_dump（allow_unicode、不排序键）字节级等价
//!   （已用 `/tmp/yamlcmp` 对真实 review.yaml 验证 IDENTICAL）。
//! - Rust 专属测试钩子：`NOVEL_REVIEW_RULES_YAML` 环境变量可覆盖规则 yaml 路径
//!   （默认仍为仓库 `configs/rules/review.yaml`）。
//! - 记录差异（PORTING_NOTES）：标量 → 字符串渲染（bool 用 JSON 小写
//!   "true"/"false"；科学计数数归一为十进制；
//!   dict/list → JSON 结构化文本而非 repr）与 catalog 缺失键的容错
//!   （缺失键处 Rust 给默认值而非报错）。

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde_json::Value;
use serde_yaml::{Mapping as YamlMapping, Sequence as YamlSequence, Value as YamlValue};

use crate::config;
use crate::stats::Ctr;

/// 三个回写目标（模块级常量；`YAML_TEMPLATE_TARGET` 值同规则 yaml 默认路径，仅作文档常量）。
pub const YAML_TEMPLATE_TARGET: &str = "configs/rules/review.yaml#draft.template_rules";
pub const YAML_TERM_TARGET: &str = "configs/rules/review.yaml#draft.tracked_terms";
pub const YAML_INACTIVE_TARGET: &str =
    "configs/rules/review.yaml#draft.inactive_template_candidates";

/// `tools-apply` 子命令参数。
#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// 模板目录 CATALOG.json 路径。
    pub catalog: std::path::PathBuf,
    /// 只预览，不改文件。
    pub dry_run: bool,
    /// 执行回写。
    pub apply: bool,
}

/// 返回 (退出码, dry-run 待打印文本)。
/// apply 模式的 print 已就地输出；错误消息走 stderr + rc=1。
pub fn run(opts: &ApplyOptions) -> Result<(i32, Option<String>)> {
    run_with_rules(opts, None)
}

/// `run`，但规则文件由调用方指定：给了就用它，没给就走环境变量或默认路径。
///
/// 回写改的就是规则文件，所以它的路径必须能指定——`--rules` 优先于
/// `NOVEL_REVIEW_RULES_YAML` 环境变量。
pub fn run_with_rules(opts: &ApplyOptions, rules: Option<&Path>) -> Result<(i32, Option<String>)> {
    if !opts.dry_run && !opts.apply {
        eprintln!("Either --dry-run or --apply must be specified.");
        return Ok((1, None));
    }
    let catalog = match load_catalog(&opts.catalog) {
        Ok(payload) => payload,
        Err(message) => {
            eprintln!("{message}");
            return Ok((1, None));
        }
    };
    let plan = build_plan(&catalog);
    let rules_path = rules_yaml_path(rules);
    if opts.apply {
        println!("Applying writeback actions...");
        match apply_writeback(&plan, &rules_path) {
            Ok(()) => {
                println!("Applied successfully.");
            }
            Err(message) => {
                eprintln!("{message}");
                return Ok((1, None));
            }
        }
        Ok((0, None))
    } else {
        Ok((0, Some(render_dry_run(&opts.catalog, &catalog, &plan))))
    }
}

// ---------------------------------------------------------------------------
// catalog 加载
// ---------------------------------------------------------------------------

/// yaml/json 按后缀分支；失败文案固定，rc=1。
fn load_catalog(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Err(format!("Catalog not found: {}", path.display()));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|exc| format!("Invalid catalog in {}: {}", path.display(), exc))?;
    let is_yaml = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yaml") | Some("yml")
    );
    let payload: Value = if is_yaml {
        let yaml_value: YamlValue = serde_yaml::from_str(&text)
            .map_err(|exc| format!("Invalid catalog in {}: {}", path.display(), exc))?;
        serde_json::to_value(yaml_value)
            .map_err(|exc| format!("Invalid catalog in {}: {}", path.display(), exc))?
    } else {
        serde_json::from_str(&text)
            .map_err(|exc| format!("Invalid catalog in {}: {}", path.display(), exc))?
    };
    if !payload
        .as_object()
        .is_some_and(|obj| obj.get("writeback_queue").is_some_and(Value::is_array))
    {
        return Err("Catalog must have a list field 'writeback_queue'.".to_string());
    }
    Ok(payload)
}

/// 标量渲染（int 十进制 / str 原样 / None→"None"）。
fn json_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// `str(item.get(key, ""))`：缺省空串；present 时按标量渲染。
fn json_field(v: &Value, key: &str) -> String {
    match v.get(key) {
        None => String::new(),
        Some(item) => json_scalar(item),
    }
}

/// `catalog.get(key, default)`（top-level）。
fn json_top(catalog: &Value, key: &str, default: &str) -> String {
    match catalog.get(key) {
        None => default.to_string(),
        Some(item) => json_scalar(item),
    }
}

// ---------------------------------------------------------------------------
// build_plan（动作分类）
// ---------------------------------------------------------------------------

fn build_plan(catalog: &Value) -> Vec<Value> {
    let mut plan: Vec<Value> = Vec::new();
    if let Some(queue) = catalog.get("writeback_queue").and_then(Value::as_array) {
        for item in queue {
            let target = json_field(item, "target");
            let state = json_field(item, "state");
            let kind = json_field(item, "kind");

            let mut action = if target.ends_with("draft.template_rules")
                || target.ends_with("draft_template_bank.json")
            {
                "template_bank"
            } else if target.ends_with("draft.tracked_terms")
                || target.ends_with("draft_term_bank.json")
            {
                "term_bank"
            } else if target.ends_with(".md") || target.contains("/rules/") {
                "guide_or_rule"
            } else if target.ends_with(".rs") {
                "script_recalibration"
            } else {
                "manual_review"
            };
            if state == "hardcoded" || state == "builtin" {
                action = "script_recalibration";
            } else if state == "bank" && (action == "template_bank" || action == "term_bank") {
                action = "bank_recalibration";
            } else if kind.starts_with("keep") {
                action = "designed_keep_review";
            }

            let mut new_item = item.clone();
            if let Some(obj) = new_item.as_object_mut() {
                obj.insert("action".to_string(), Value::String(action.to_string()));
            }
            plan.push(new_item);
        }
    }
    plan
}

// ---------------------------------------------------------------------------
// dry-run 渲染（逐行稳定输出）
// ---------------------------------------------------------------------------

fn render_dry_run(catalog_path: &Path, catalog: &Value, plan: &[Value]) -> String {
    let mut lines: Vec<String> = vec![
        "# Template Candidate Writeback Dry Run".to_string(),
        String::new(),
    ];
    lines.push(format!("- catalog: `{}`", catalog_path.display()));
    lines.push(format!(
        "- novel: `{}`",
        json_top(catalog, "novel", "unknown")
    ));
    lines.push(format!(
        "- stories: `{}`",
        json_top(catalog, "stories", "0")
    ));
    lines.push(format!("- queue_items: `{}`", plan.len()));
    lines.push("- mode: `dry-run` (no files modified)".to_string());
    lines.push(String::new());

    let mut by_action: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    let mut by_state: Ctr = Ctr::default();
    for item in plan {
        let action = json_field(item, "action");
        by_action.entry(action).or_default().push(item);
        let state = json_field(item, "state");
        by_state.add(&state, 1);
    }

    lines.push("## State Summary".to_string());
    // 计数降序，并列保持首次出现序。
    for (state, count) in by_state.most_common_all() {
        lines.push(format!("- `{state}` x{count}"));
    }
    lines.push(String::new());

    let order = [
        "template_bank",
        "term_bank",
        "bank_recalibration",
        "designed_keep_review",
        "script_recalibration",
        "guide_or_rule",
        "manual_review",
    ];
    let title_for = |action: &str| -> String {
        match action {
            "template_bank" => format!("Would add to {YAML_INACTIVE_TARGET}"),
            "term_bank" => format!("Would add to {YAML_TERM_TARGET}"),
            "bank_recalibration" => "Would recalibrate existing bank entries".into(),
            "designed_keep_review" => "Needs designed-keep review".into(),
            "script_recalibration" => "Needs script recalibration".into(),
            "guide_or_rule" => "Would update guide or rule docs".into(),
            "manual_review" => "Needs manual routing".into(),
            _ => String::new(),
        }
    };
    for action in order {
        let items = by_action.get(action).cloned().unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        lines.push(format!("## {}", title_for(action)));
        for item in &items {
            let stories = json_field(item, "stories");
            let count = json_field(item, "count");
            lines.push(format!(
                "- `{}` `{}` `{}` -> `{}` stories=`{stories}` count=`{count}`",
                json_field(item, "state"),
                json_field(item, "kind"),
                json_field(item, "name"),
                json_field(item, "target")
            ));
            lines.push(format!("  reason: {}", json_field(item, "reason")));
        }
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n").trim_end())
}

// ---------------------------------------------------------------------------
// apply 写回（template bank / term bank 读改写）
// ---------------------------------------------------------------------------

/// 规则 yaml 路径：`NOVEL_REVIEW_RULES_YAML` 可覆盖（测试钩子），默认仓库 review.yaml。
/// 要回写的规则文件：显式指定的优先，其次环境变量，最后默认路径。
fn rules_yaml_path(rules: Option<&Path>) -> std::path::PathBuf {
    if let Some(path) = rules {
        return path.to_path_buf();
    }
    std::env::var("NOVEL_REVIEW_RULES_YAML")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(config::default_rules_path)
}

/// 解析失败 / 非 dict → `Invalid review rules YAML: {path}`。
fn load_rules_yaml(path: &Path) -> Result<YamlValue, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|exc| format!("Invalid review rules YAML: {}: {exc}", path.display()))?;
    let payload: YamlValue = serde_yaml::from_str(&text)
        .map_err(|exc| format!("Invalid review rules YAML: {}: {exc}", path.display()))?;
    if payload.as_mapping().is_none() {
        return Err(format!("Invalid review rules YAML: {}", path.display()));
    }
    Ok(payload)
}

/// 全量覆写（safe_dump 语义：allow_unicode、不排序键）。
fn write_rules_yaml(path: &Path, payload: &YamlValue) -> Result<(), String> {
    let text = serde_yaml::to_string(payload)
        .map_err(|exc| format!("Failed to write rules YAML {}: {exc}", path.display()))?;
    std::fs::write(path, text)
        .map_err(|exc| format!("Failed to write rules YAML {}: {exc}", path.display()))?;
    Ok(())
}

/// 在 mapping 上 `setdefault(key, value)`（键缺失才插入）。
fn yaml_setdefault<'a>(
    map: &'a mut YamlMapping,
    key: &str,
    default: YamlValue,
) -> &'a mut YamlValue {
    if map.get_mut(key).is_none() {
        map.insert(key.into(), default);
    }
    map.get_mut(key).expect("key 已确保存在")
}

/// `float(item.get(key, default))`：JSON 数值→f64；字符串解析；缺省→默认。
fn json_float(v: Option<&Value>, default: f64) -> f64 {
    match v {
        None => default,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
        Some(Value::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

/// 新 template 条目的 note / category / max_per_10k 标量。
fn json_str_or(v: Option<&Value>, default: &str) -> String {
    match v {
        None => default.to_string(),
        Some(item) => json_scalar(item),
    }
}

fn note_value_chain(item: &Value, existing_note: Option<&YamlValue>) -> String {
    // 回退序：item.reason → item.note → existing.note → 空串
    if item.get("reason").is_some() {
        json_scalar(item.get("reason").expect("reason present"))
    } else if item.get("note").is_some() {
        json_scalar(item.get("note").expect("note present"))
    } else {
        match existing_note {
            Some(YamlValue::String(s)) => s.clone(),
            _ => String::new(),
        }
    }
}

fn apply_to_template_bank(item: &Value, rules_path: &Path) -> Result<(), String> {
    let mut payload = load_rules_yaml(rules_path)?;
    let mapping = payload.as_mapping_mut().expect("dict 已校验");
    let draft = yaml_setdefault(mapping, "draft", YamlValue::Mapping(YamlMapping::new()));
    if draft.as_mapping().is_none() {
        return Err("Invalid draft section in review rules YAML".to_string());
    }
    let draft_map = draft.as_mapping_mut().expect("draft 为 dict");
    let inactive = yaml_setdefault(
        draft_map,
        "inactive_template_candidates",
        YamlValue::Sequence(YamlSequence::new()),
    );
    if inactive.as_sequence().is_none() {
        return Err("Invalid inactive_template_candidates section".to_string());
    }
    let inactive_seq = inactive.as_sequence_mut().expect("inactive 为 list");

    let name = json_field(item, "name");
    let existing = inactive_seq.iter_mut().find(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get("name"))
            .and_then(YamlValue::as_str)
            == Some(name.as_str())
    });
    if let Some(existing_map) = existing.and_then(|e| e.as_mapping_mut()) {
        if item.get("note").is_some() || item.get("reason").is_some() {
            let existing_note = existing_map.get("note");
            let note = note_value_chain(item, existing_note);
            existing_map.insert("note".into(), YamlValue::String(note));
        }
    } else {
        let mut entry = YamlMapping::new();
        entry.insert("name".into(), YamlValue::String(name.clone()));
        entry.insert("pattern".into(), YamlValue::String(String::new()));
        entry.insert("note".into(), YamlValue::String(json_field(item, "reason")));
        entry.insert(
            "max_per_10k".into(),
            YamlValue::from(json_float(item.get("max_per_10k"), 5.0)),
        );
        entry.insert(
            "category".into(),
            YamlValue::String(json_str_or(item.get("category"), "auto_generated")),
        );
        entry.insert("enabled".into(), YamlValue::Bool(false));
        inactive_seq.push(YamlValue::Mapping(entry));
    }
    write_rules_yaml(rules_path, &payload)?;
    println!("Added to {YAML_INACTIVE_TARGET}: {name}");
    Ok(())
}

fn apply_to_term_bank(item: &Value, rules_path: &Path) -> Result<(), String> {
    let mut payload = load_rules_yaml(rules_path)?;
    let mapping = payload.as_mapping_mut().expect("dict 已校验");
    let draft = yaml_setdefault(mapping, "draft", YamlValue::Mapping(YamlMapping::new()));
    if draft.as_mapping().is_none() {
        return Err("Invalid draft section in review rules YAML".to_string());
    }
    let draft_map = draft.as_mapping_mut().expect("draft 为 dict");
    let terms = yaml_setdefault(
        draft_map,
        "tracked_terms",
        YamlValue::Sequence(YamlSequence::new()),
    );
    if terms.as_sequence().is_none() {
        return Err("Invalid tracked_terms section".to_string());
    }
    let terms_seq = terms.as_sequence_mut().expect("terms 为 list");

    let name = json_field(item, "name");
    let existing = terms_seq.iter_mut().find(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get("term"))
            .and_then(YamlValue::as_str)
            == Some(name.as_str())
    });
    if let Some(existing_map) = existing.and_then(|e| e.as_mapping_mut()) {
        if item.get("note").is_some() || item.get("reason").is_some() {
            let existing_note = existing_map.get("note");
            let note = note_value_chain(item, existing_note);
            existing_map.insert("note".into(), YamlValue::String(note));
        }
        if item.get("max_per_10k").is_some() {
            existing_map.insert(
                "max_per_10k".into(),
                YamlValue::from(json_float(item.get("max_per_10k"), 10.0)),
            );
        }
        if item.get("category").is_some() {
            existing_map.insert(
                "category".into(),
                YamlValue::String(json_scalar(item.get("category").expect("category present"))),
            );
        }
    } else {
        let mut entry = YamlMapping::new();
        entry.insert("term".into(), YamlValue::String(name.clone()));
        entry.insert(
            "category".into(),
            YamlValue::String(json_str_or(item.get("category"), "auto_generated")),
        );
        entry.insert(
            "max_per_10k".into(),
            YamlValue::from(json_float(item.get("max_per_10k"), 10.0)),
        );
        entry.insert(
            "note".into(),
            YamlValue::String(note_value_chain(item, None)),
        );
        terms_seq.push(YamlValue::Mapping(entry));
    }
    write_rules_yaml(rules_path, &payload)?;
    println!("Added to {YAML_TERM_TARGET}: {name}");
    Ok(())
}

/// template/term 回写，其余打印 skip。
fn apply_writeback(plan: &[Value], rules_path: &Path) -> Result<(), String> {
    for item in plan {
        let action = json_field(item, "action");
        match action.as_str() {
            "template_bank" => apply_to_template_bank(item, rules_path)?,
            "term_bank" => apply_to_term_bank(item, rules_path)?,
            _ => {
                let name = json_field(item, "name");
                println!("Skipping action: {action} for {name} (not implemented)");
            }
        }
    }
    Ok(())
}
