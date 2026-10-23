//! 共享小工具（计数器/标签摘要、路径相对化、章节序解析）。

use super::*;

/// `name xN` 空格连排摘要，空为「无」。
pub(crate) fn summarize_counter(counter: &Ctr, limit: usize) -> String {
    let joined = counter
        .most_common(limit)
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        "无".to_string()
    } else {
        joined
    }
}

/// 连续标签摘要（workspace 本地版，
/// 与 `stats.draft.summarize_runs` 不同：不带展示名映射、带 ch 区间、排除弱标签）。
pub(crate) fn summarize_runs_local(labels_in_order: &[String]) -> Vec<String> {
    if labels_in_order.is_empty() {
        return Vec::new();
    }
    let mut runs: Vec<String> = Vec::new();
    let mut current = &labels_in_order[0];
    let mut start = 1usize;
    let mut length = 1usize;
    for (offset, label) in labels_in_order[1..].iter().enumerate() {
        let index = offset + 2;
        if label == current {
            length += 1;
            continue;
        }
        if !matches!(current.as_str(), "unclear" | "missing" | "none" | "neutral") && length >= 3 {
            runs.push(format!(
                "{current} x{length} (ch{start:02}-ch{end:02})",
                end = index - 1
            ));
        }
        current = label;
        start = index;
        length = 1;
    }
    if !matches!(current.as_str(), "unclear" | "missing" | "none" | "neutral") && length >= 3 {
        runs.push(format!(
            "{current} x{length} (ch{start:02}-ch{end:02})",
            end = start + length - 1
        ));
    }
    runs.truncate(4);
    runs
}

/// 结局信号/基调/情绪三流汇聚点摘要行。
pub(crate) fn summarize_story_convergences(
    ending_signal_flow: &[String],
    tone_flow: &[String],
    emotion_flow: &[String],
    labels: &EndingLabels,
) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for idx in 0..ending_signal_flow.len().saturating_sub(1) {
        if ending_signal_flow[idx] != ending_signal_flow[idx + 1] {
            continue;
        }
        if tone_flow[idx] == tone_flow[idx + 1] && tone_flow[idx] != "none" {
            items.push(format!(
                "{}+tone:{} x2",
                ending_display(labels, &ending_signal_flow[idx]),
                tone_flow[idx]
            ));
        }
        if emotion_flow[idx] == emotion_flow[idx + 1] && emotion_flow[idx] != "neutral" {
            items.push(format!(
                "{}+emotion:{} x2",
                ending_display(labels, &ending_signal_flow[idx]),
                emotion_flow[idx]
            ));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen.truncate(4);
    seen
}

/// `story_dir.relative_to(drafts_dir).as_posix()`：自身相对为 `.`；越界时退回原样。
pub(crate) fn rel_posix(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(p) if p.as_os_str().is_empty() => ".".to_string(),
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// 由路径解析章节序与章名：`(order, chapter_name)`。
pub(crate) fn chapter_order_from_path(path_text: &str, novel_dir: &Path) -> (i64, String) {
    let path = Path::new(path_text);
    let (_doc_type, _arc, _story, chapter) = consistency::classify_document(path, novel_dir);
    let chapter_name = chapter.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    if let Ok(Some(caps)) = CHAPTER_ID_RE.captures(&chapter_name) {
        let n = caps
            .get(1)
            .and_then(|m| m.as_str().parse::<i64>().ok())
            .unwrap_or(9999);
        return (n, chapter_name);
    }
    (9999, chapter_name)
}
