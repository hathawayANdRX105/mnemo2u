//! `study.compare`：对比两份 analysis JSON，输出 Markdown 指标差异表。
//!
//! 行为：
//! - `schema_version` 不等 → stderr `Error: schema_version mismatch: baseline={bv}, applied={av}` rc=1
//! - 提取 `summary` 顶层 int/float 与二级 dict 内 int/float → Markdown 表
//! - 表头/分隔行/数值渲染：int 不带 `.0`，float 走 `audit::draft::float_repr`
//! - 非数字 `summary` 键逐行 `不参与比较`
//! - 缺 `summary` 键容忍（视为空）
//!
//! `audit-draft --format json` 输出顶层数组
//! `[{source, summary, ...}]`；本模块按意图取数组首元素，空数组报错。

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::Value;

/// 读取并解析 JSON 文件。
fn load_json(path: &std::path::Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 JSON 文件失败: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("JSON 解析失败: {}", path.display()))
}

/// `audit-draft --format json` 的顶层数组报告归一为单份 analysis（取首元素）。
fn unwrap_report(value: Value) -> Result<Value> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .next()
            .context("JSON 顶层为空数组，无 analysis 可比较"),
        other => Ok(other),
    }
}

/// 取 `analysis` 中的 `schema_version`（缺键或 null → `None`）。
fn schema_version(analysis: &Value) -> Option<&Value> {
    let v = analysis.get("schema_version")?;
    (v != &Value::Null).then_some(v)
}

/// `schema_version` 渲染：缺键/None → `None` 字符串，
/// bool → `True`/`False`，str 原样（不带引号），number 走 JSON 表示。
fn schema_version_repr(v: Option<&Value>) -> String {
    match v {
        None => "None".to_string(),
        Some(Value::Bool(b)) => (if *b { "True" } else { "False" }).to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(value) => value.to_string(),
    }
}

/// 提取 `summary` 中顶层数字指标与二级 dict 内数字指标。
///
/// 返回按指标名排序的有序表（`BTreeMap`），键为 `"k"` 或 `"k.kk"`，
/// 值为 `(f64, bool)` —— 浮点值 + 是否为 int。
fn extract_numeric_metrics(analysis: &Value) -> BTreeMap<String, (f64, bool)> {
    let mut out: BTreeMap<String, (f64, bool)> = BTreeMap::new();
    if let Some(summary) = analysis.get("summary").and_then(|v| v.as_object()) {
        for (k, v) in summary {
            if let Some(f) = v.as_i64() {
                out.insert(k.clone(), (f as f64, true));
            } else if let Some(f) = v.as_f64() {
                out.insert(k.clone(), (f, false));
            } else if let Some(nested) = v.as_object() {
                for (kk, vv) in nested {
                    if let Some(fi) = vv.as_i64() {
                        out.insert(format!("{k}.{kk}"), (fi as f64, true));
                    } else if let Some(ff) = vv.as_f64() {
                        out.insert(format!("{k}.{kk}"), (ff, false));
                    }
                }
            }
        }
    }
    out
}

/// 收集 `summary` 中所有键名（顶层非 dict 键 + 二级 dict 键）。
/// 返回排序后的去重键名列表。
fn all_summary_keys(analysis: &Value) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut all: BTreeSet<String> = BTreeSet::new();
    if let Some(summary) = analysis.get("summary").and_then(|v| v.as_object()) {
        for (k, v) in summary {
            if v.is_object() {
                if let Some(nested) = v.as_object() {
                    for kk in nested.keys() {
                        all.insert(format!("{k}.{kk}"));
                    }
                }
            } else {
                all.insert(k.clone());
            }
        }
    }
    all.into_iter().collect()
}

/// 内部辅助：由 `(f64, is_int)` 渲染数字（int 不带 `.0`，float 走 `float_repr`）。
fn render_number_by_f64(val: f64, is_int: bool) -> String {
    if is_int {
        (val as i64).to_string()
    } else {
        crate::audit::draft::float_repr(val)
    }
}

/// 渲染 delta：两侧 int 且差为整 → 整数字符串；否则按 float 渲染。
fn render_delta(b: f64, b_is_int: bool, a: f64, a_is_int: bool) -> String {
    let delta = a - b;
    if b_is_int && a_is_int && delta.fract() == 0.0 {
        (delta as i64).to_string()
    } else {
        crate::audit::draft::float_repr(delta)
    }
}

/// 构建对比 Markdown 表。
///
/// 调用方须先完成 `schema_version` 校验。
pub fn build_compare_table(baseline: &Value, applied: &Value) -> String {
    let b_metrics = extract_numeric_metrics(baseline);
    let a_metrics = extract_numeric_metrics(applied);

    // 共同数字指标（两边都有同名键）
    let shared: Vec<String> = b_metrics
        .keys()
        .filter(|k| a_metrics.contains_key(*k))
        .cloned()
        .collect();

    // 表头 + 分隔行
    let mut lines: Vec<String> = vec![
        "| Metric | Baseline | Applied | Delta |".to_string(),
        "|--------|----------|---------|-------|".to_string(),
    ];

    for metric in &shared {
        let (bv, b_int) = b_metrics.get(metric).unwrap();
        let (av, a_int) = a_metrics.get(metric).unwrap();
        lines.push(format!(
            "| {} | {} | {} | {} |",
            metric,
            render_number_by_f64(*bv, *b_int),
            render_number_by_f64(*av, *a_int),
            render_delta(*bv, *b_int, *av, *a_int)
        ));
    }

    // 非数字 summary 键（顶层非 dict 键 + 二级 dict 键，两边并集）
    let b_numeric_keys: std::collections::BTreeSet<&str> =
        b_metrics.keys().map(|s| s.as_str()).collect();
    let a_numeric_keys: std::collections::BTreeSet<&str> =
        a_metrics.keys().map(|s| s.as_str()).collect();

    let b_all = all_summary_keys(baseline);
    let a_all = all_summary_keys(applied);
    let all_keys: std::collections::BTreeSet<String> =
        b_all.iter().chain(a_all.iter()).cloned().collect();

    let numeric_keys: std::collections::BTreeSet<&str> = b_numeric_keys
        .iter()
        .chain(a_numeric_keys.iter())
        .copied()
        .collect();

    let non_numeric: Vec<String> = all_keys
        .into_iter()
        .filter(|k| !numeric_keys.contains(k.as_str()))
        .collect();

    for key in &non_numeric {
        lines.push(format!(
            "| {} | 不参与比较 | 不参与比较 | 不参与比较 |",
            key
        ));
    }

    lines.join("\n")
}

/// 运行 `study-compare` 子命令。
///
/// 成功 → `Ok(table)`；`schema_version` 不等 → `Err("schema_version mismatch: baseline={bv}, applied={av}")`。
pub fn run_compare(
    baseline_path: &std::path::Path,
    applied_path: &std::path::Path,
) -> Result<String> {
    let baseline = unwrap_report(load_json(baseline_path)?)?;
    let applied = unwrap_report(load_json(applied_path)?)?;

    let bv = schema_version(&baseline);
    let av = schema_version(&applied);
    if bv != av {
        anyhow::bail!(
            "schema_version mismatch: baseline={}, applied={}",
            schema_version_repr(bv),
            schema_version_repr(av)
        );
    }

    Ok(build_compare_table(&baseline, &applied))
}
