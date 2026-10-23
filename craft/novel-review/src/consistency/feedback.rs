//! 反馈 JSONL 读写与沉淀建议。

use super::*;
// ---------------------------------------------------------------------------
// 反馈 JSONL
// ---------------------------------------------------------------------------

/// `read_feedback_history`：逐行 JSON，值字符串化（None 丢弃）。
pub fn read_feedback_history(path: &Path) -> Result<Vec<FeedbackRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let mut history = Vec::new();
    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let data: FeedbackRecord = serde_json::from_str(line)?;
        let mut record: FeedbackRecord = JsonMap::new();
        for (key, value) in data {
            let stringified = match value {
                JsonValue::Null => continue,
                JsonValue::String(s) => JsonValue::String(s),
                JsonValue::Bool(b) => JsonValue::String(b.to_string()),
                other => JsonValue::String(other.to_string()),
            };
            record.insert(key, stringified);
        }
        history.push(record);
    }
    Ok(history)
}

pub(crate) fn record_str(record: &FeedbackRecord, key: &str) -> String {
    record
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// `load_feedback_entries`：`conflict_key` 重复时后写覆盖。
pub fn load_feedback_entries(path: &Path) -> Result<BTreeMap<String, FeedbackRecord>> {
    let mut entries: BTreeMap<String, FeedbackRecord> = BTreeMap::new();
    for data in read_feedback_history(path)? {
        let key = record_str(&data, "conflict_key").trim().to_string();
        if key.is_empty() {
            continue;
        }
        entries.insert(key, data);
    }
    Ok(entries)
}

/// `append_feedback_entry`（JSON 默认分隔符 `": "` / `", "`；
/// 键序 = entry 插入序，与 map 后端无关）。
pub fn append_feedback_entry(path: &Path, entry: &FeedbackRecord) -> Result<()> {
    const KEY_ORDER: &[&str] = &[
        "conflict_key",
        "category",
        "story",
        "title",
        "entity_category",
        "summary",
        "decision",
        "facet",
        "note",
        "updated_at",
    ];
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    let mut line = String::from("{");
    let mut first = true;
    for key in KEY_ORDER {
        let Some(value) = entry.get(*key) else {
            continue;
        };
        if !first {
            line.push_str(", ");
        }
        first = false;
        line.push_str(&serde_json::to_string(key)?);
        line.push_str(": ");
        line.push_str(&serde_json::to_string(value)?);
    }
    // 兼容未知键（read-back 再写场景）：按字节序排尾。
    let known: std::collections::BTreeSet<&str> = KEY_ORDER.iter().copied().collect();
    let extra: Vec<(String, &JsonValue)> = entry
        .iter()
        .filter(|(k, _)| !known.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v))
        .collect();
    for (key, value) in extra {
        if !first {
            line.push_str(", ");
        }
        first = false;
        line.push_str(&serde_json::to_string(&key)?);
        line.push_str(": ");
        line.push_str(&serde_json::to_string(value)?);
    }
    line.push('}');
    line.push('\n');
    use std::io::Write;
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    handle.write_all(line.as_bytes())?;
    Ok(())
}

/// `build_feedback_backlog`。
pub fn build_feedback_backlog(history: &[FeedbackRecord]) -> Vec<BacklogItem> {
    let mut category_decision = Ctr::default();
    let mut story_decision = Ctr::default();
    let mut facet = Ctr::default();
    for entry in history {
        let category = record_str(entry, "category");
        let story = record_str(entry, "story");
        let decision = record_str(entry, "decision");
        let facet_value = record_str(entry, "facet");
        if !category.is_empty() && !decision.is_empty() {
            category_decision.add(&format!("{category}::{decision}"), 1);
        }
        if !story.is_empty() && !decision.is_empty() {
            story_decision.add(&format!("{story}::{decision}"), 1);
        }
        if decision == "designed_keep" && !facet_value.is_empty() {
            facet.add(&facet_value, 1);
        }
    }

    let top_by_suffix = |counter: &Ctr, suffix: &str| -> Option<(String, usize)> {
        counter
            .most_common_all()
            .into_iter()
            .filter(|(name, _)| name.ends_with(suffix))
            .min_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)))
    };

    let mut backlog: Vec<BacklogItem> = Vec::new();

    if let Some((top_name, top_count)) = top_by_suffix(&category_decision, "::false_positive") {
        let category = top_name.split("::").next().unwrap_or(&top_name);
        backlog.push(BacklogItem {
            target: CONSISTENCY_MODULE_TARGET.to_string(),
            reason: format!(
                "`{category}` 已累计 {top_count} 条误报反馈，优先压抽取噪声，不要继续把人工复核当默认补丁。"
            ),
        });
    }

    if let Some((top_name, top_count)) = top_by_suffix(&category_decision, "::designed_keep") {
        let category = top_name.split("::").next().unwrap_or(&top_name);
        let mut target = RULES_TEMPLATE_TARGET.to_string();
        let mut reason_tail = "说明这类变化应开始沉淀为可保留模式样本。".to_string();
        if let Some((top_facet, facet_count)) = facet.most_common(1).into_iter().next() {
            if top_facet == "register" || top_facet == "naming" {
                target = BOOK_DRAFT_RULES_TARGET.to_string();
                reason_tail = format!(
                    "其中 `{top_facet}` 已出现 {facet_count} 次，更适合先写成命名/称谓边界规则。"
                );
            } else if matches!(
                top_facet.as_str(),
                "voice" | "rhythm" | "motif" | "scene_callback" | "irony"
            ) {
                target = RULES_TEMPLATE_TARGET.to_string();
                reason_tail = format!(
                    "其中 `{top_facet}` 已出现 {facet_count} 次，应开始积累这类可保留风格样本。"
                );
            }
        }
        backlog.push(BacklogItem {
            target,
            reason: format!("`{category}` 已累计 {top_count} 条设计性保留反馈，{reason_tail}"),
        });
    }

    if let Some((top_name, top_count)) = top_by_suffix(&story_decision, "::confirmed") {
        let story = top_name.split("::").next().unwrap_or(&top_name);
        backlog.push(BacklogItem {
            target: BOOK_DRAFT_RULES_TARGET.to_string(),
            reason: format!(
                "`{story}` 已累计 {top_count} 条确认成立的一致性问题，说明这不是偶发手误，值得沉淀为返工规则。"
            ),
        });
    }

    if let Some((top_name, top_count)) = top_by_suffix(&story_decision, "::watch") {
        let story = top_name.split("::").next().unwrap_or(&top_name);
        backlog.push(BacklogItem {
            target: "novel1/research/consistency/review-feedback.jsonl".to_string(),
            reason: format!("`{story}` 仍有 {top_count} 条长期待观察反馈，说明这条 Story 的一致性口径还没真正收敛。"),
        });
    }

    backlog.truncate(6);
    backlog
}

/// `filter_feedback_history_by_story`。
pub fn filter_feedback_history_by_story(
    history: &[FeedbackRecord],
    story: Option<&str>,
) -> Vec<FeedbackRecord> {
    match story {
        None => history.to_vec(),
        Some(story) => history
            .iter()
            .filter(|entry| record_str(entry, "story") == story)
            .cloned()
            .collect(),
    }
}
