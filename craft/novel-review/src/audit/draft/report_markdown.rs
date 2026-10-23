//! Markdown 报告渲染与 CLI 入口（`format_markdown_report` / `RunOptions` / `run`，输出逐字节稳定）。

use super::*;

// ---------------------------------------------------------------------------
// Markdown 报告

/// Markdown 表格单元格净化：换行 → 空格，`|` → `\|`（对齐 `_markdown_table_cell`）。
fn markdown_table_cell(value: &str) -> String {
    value.replace('\n', " ").replace('|', "\\|")
}

/// `add_metric_section` 的一行指标（各指标类型字段形状一致）。
struct MdMetric<'a> {
    name: &'a str,
    count: usize,
    per_10k: f64,
    max_per_10k: f64,
    warn: bool,
    samples: &'a [Hit],
}

/// 渲染一条规则指标小节。
fn metric_section_lines(header: &str, metrics: &[MdMetric<'_>]) -> Vec<String> {
    let mut lines = vec![format!("## {header}")];
    if metrics.is_empty() {
        lines.push("- 无".into());
        lines.push(String::new());
        return lines;
    }
    for metric in metrics {
        let status = if metric.warn { "WARN" } else { "OK" };
        lines.push(format!(
            "- `{status}` `{}` count=`{}` per_10k=`{}` max=`{}`",
            metric.name,
            metric.count,
            float_repr(metric.per_10k),
            float_repr(metric.max_per_10k)
        ));
        if let Some(sample) = metric.samples.first() {
            lines.push(format!("  样例：`L{}` {}", sample.line_no, sample.snippet));
        }
    }
    lines.push(String::new());
    lines
}

/// 规则指标行 → `MdMetric` 行（`RegexMetric` 与 markdown 小节同形状）。
fn regex_metric_rows(items: &[crate::rules::RegexMetric]) -> Vec<MdMetric<'_>> {
    items
        .iter()
        .map(|m| MdMetric {
            name: &m.name,
            count: m.count,
            per_10k: m.per_10k,
            max_per_10k: m.max_per_10k,
            warn: m.warn,
            samples: &m.samples,
        })
        .collect()
}

/// 把完整 analysis 渲染成 Markdown 报告。
/// `title` 为 None 时回退 `analysis.source`。
#[must_use]
pub fn format_markdown_report(a: &Analysis, title: Option<&str>) -> String {
    let s = &a.summary;
    let title = title.unwrap_or(&a.source);
    let ending_image_md = a
        .ending
        .image_terms
        .iter()
        .map(|t| format!("{} x{}", t.term, t.count))
        .collect::<Vec<_>>()
        .join(", ");
    let ending_flow_md = a
        .ending
        .flow_terms
        .iter()
        .map(|t| format!("{} x{}", t.term, t.count))
        .collect::<Vec<_>>()
        .join(", ");
    let mut lines: Vec<String> = vec![format!("# {title}"), String::new()];

    // ## 概览
    lines.push("## 概览".into());
    lines.push(format!("- 来源：`{}`", a.source));
    lines.push(format!("- 字数：`{}`", s.chars));
    lines.push(format!("- 句子数：`{}`", s.sentences));
    lines.push(format!("- 段落数：`{}`", s.paragraphs));
    lines.push(format!(
        "- 句均字数：`{}`",
        float_repr(s.avg_sentence_chars)
    ));
    lines.push(format!("- 短句数：`{}`", s.short_sentences));
    lines.push(format!("- 极短句数：`{}`", s.very_short_sentences));
    lines.push(format!(
        "- 短句占比：`{}`",
        float_repr(s.short_sentence_ratio)
    ));
    lines.push(format!("- 引号占比：`{}`", float_repr(s.quote_ratio)));
    lines.push(format!("- 警告分区数：`{}`", s.warn_sections));
    lines.push(format!(
        "- 总体状态：`{}`",
        if a.warned { "WARN" } else { "OK" }
    ));
    lines.push(String::new());

    // ## 审查提醒
    lines.push("## 审查提醒".into());
    if !a.review_reminders.is_empty() {
        for item in a.review_reminders.iter().take(8) {
            let mut line = format!(
                "- `{}` `{}` {}：{} 检查：{} 动作：{}",
                item.priority, item.category, item.title, item.reason, item.check, item.action
            );
            if !item.evidence.is_empty() {
                let evidence = item
                    .evidence
                    .iter()
                    .map(|v| markdown_table_cell(v))
                    .collect::<Vec<_>>()
                    .join("；");
                line.push_str(&format!(" 证据：{evidence}"));
            }
            lines.push(line);
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    // ## 句式疲劳雷达
    lines.push("## 句式疲劳雷达".into());
    if !a.style_fatigue.is_empty() {
        lines.push("| 状态 | 句式家族 | 数量 | 风险 | 减少方式 | 证据 |".into());
        lines.push("|---|---|---:|---|---|---|".into());
        for item in &a.style_fatigue {
            let joined = item
                .evidence
                .iter()
                .map(|v| markdown_table_cell(v))
                .collect::<Vec<_>>()
                .join("；");
            let evidence = if joined.is_empty() {
                "无".into()
            } else {
                joined
            };
            lines.push(format!(
                "| `{}` | {} | `{}` | {} | {} | {evidence} |",
                item.status,
                markdown_table_cell(&item.family),
                item.count,
                markdown_table_cell(&item.risk),
                markdown_table_cell(&item.reduce)
            ));
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    // ## 局部疲劳窗口
    lines.push("## 局部疲劳窗口".into());
    if !a.fatigue_windows.is_empty() {
        let shown = a.fatigue_windows.len().min(8);
        lines.push(format!(
            "- 命中总数：`{}`；展示：`{}`",
            a.fatigue_window_count, shown
        ));
        for item in a.fatigue_windows.iter().take(shown) {
            let roles = format_short_roles(&item.roles, "，");
            lines.push(format!(
                "- `S{}-{}` `L{}-{}` score=`{}`：{}",
                item.start_index,
                item.end_index,
                item.start_line,
                item.end_line,
                item.score,
                markdown_table_cell(&item.reasons.join("、"))
            ));
            if !roles.is_empty() {
                lines.push(format!("  类型：{}", markdown_table_cell(&roles)));
            }
            if !item.suggestion.is_empty() {
                lines.push(format!("  建议：{}", markdown_table_cell(&item.suggestion)));
            }
            let sample = item
                .sample
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            lines.push(format!("  样例：{}", markdown_table_cell(&sample)));
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    // ## 把字操作分类
    lines.push("## 把字操作分类".into());
    if !a.ba_operation_contexts.is_empty() {
        for item in &a.ba_operation_contexts {
            lines.push(format!(
                "- `{}` `{}` count=`{}`：{}",
                if item.warn { "WARN" } else { "WATCH" },
                item.role,
                item.count,
                markdown_table_cell(&item.suggestion)
            ));
            for sample in item.samples.iter().take(5) {
                lines.push(format!(
                    "  - `S{}` `L{}` `{}`：{}",
                    sample.index,
                    sample.line_no,
                    sample.snippet,
                    markdown_table_cell(&sample.sentence)
                ));
            }
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    // ## 优先修项
    lines.push("## 优先修项".into());
    if !a.hard_flags.is_empty() {
        for item in a.hard_flags.iter().take(15) {
            let mut line = format!(
                "- `{}` `{}` x{}：{}",
                item.section, item.name, item.count, item.note
            );
            if let Some(per_10k) = item.per_10k {
                line.push_str(&format!("；per_10k=`{}`", float_repr(per_10k)));
            }
            if item.register != "neutral" {
                line.push_str(&format!("；语域=`{}`", item.register));
            }
            if !item.sample.is_empty() {
                line.push_str(&format!("；样例：{}", item.sample));
            }
            lines.push(line);
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    lines.extend(metric_section_lines(
        "高频词",
        &regex_metric_rows(&a.tokens),
    ));
    let tracked_rows: Vec<MdMetric> = a
        .tracked_terms
        .iter()
        .map(|m| MdMetric {
            name: &m.name,
            count: m.count,
            per_10k: m.per_10k,
            max_per_10k: m.max_per_10k,
            warn: m.warn,
            samples: &m.samples,
        })
        .collect();
    lines.extend(metric_section_lines("跟踪词", &tracked_rows));
    lines.extend(metric_section_lines(
        "模板句",
        &regex_metric_rows(&a.patterns),
    ));
    lines.extend(metric_section_lines(
        "短触发词",
        &regex_metric_rows(&a.phrases),
    ));
    lines.extend(metric_section_lines(
        "黏糊词与判断副词",
        &regex_metric_rows(&a.modifiers),
    ));
    lines.extend(metric_section_lines(
        "标点",
        &regex_metric_rows(&a.punctuation),
    ));
    lines.extend(metric_section_lines(
        "组合标点",
        &regex_metric_rows(&a.punctuation_combos),
    ));
    let custom_rows: Vec<MdMetric> = a
        .custom_templates
        .iter()
        .map(|m| MdMetric {
            name: &m.name,
            count: m.count,
            per_10k: m.per_10k,
            max_per_10k: m.max_per_10k,
            warn: m.warn,
            samples: &m.samples,
        })
        .collect();
    lines.extend(metric_section_lines("模板库命中", &custom_rows));
    let learned_rows: Vec<MdMetric> = a
        .learned_filters
        .iter()
        .map(|m| MdMetric {
            name: &m.name,
            count: m.count,
            per_10k: m.per_10k,
            max_per_10k: m.max_per_10k,
            warn: m.warn,
            samples: &m.samples,
        })
        .collect();
    lines.extend(metric_section_lines("语料学习筛选", &learned_rows));

    // ## 语料学习基线
    lines.push("## 语料学习基线".into());
    let p = &a.corpus_profile;
    if p.enabled {
        lines.push(format!(
            "- 学习来源：`{}` 个文件，语料字数=`{}`，草稿字数=`{}`",
            p.source_count, p.chars, p.draft_chars
        ));
        if let BaselineJson::Values(baseline) = &p.sentence_length_baseline {
            lines.push(format!(
                "- 草稿句长基线：p10=`{}` p25=`{}` median=`{}` avg=`{}` short_ratio=`{}`",
                baseline.p10_chars,
                baseline.p25_chars,
                baseline.median_chars,
                float_repr(baseline.avg_chars),
                float_repr(baseline.short_ratio)
            ));
        }
        if !p.learned_sentence_leads.is_empty() {
            let leads = p
                .learned_sentence_leads
                .iter()
                .take(8)
                .map(|i| format!("{} x{}", i.phrase, i.count))
                .collect::<Vec<_>>()
                .join("，");
            lines.push(format!("- 学到的句首高频：{leads}"));
        }
        if !p.learned_aa_bb_shapes.is_empty() {
            let shapes = p
                .learned_aa_bb_shapes
                .iter()
                .take(8)
                .map(|i| format!("{} x{}", i.name, i.count))
                .collect::<Vec<_>>()
                .join("，");
            lines.push(format!("- 学到的 AA/BB 风险：{shapes}"));
        }
    } else {
        lines.push("- 未启用".into());
    }
    lines.push(String::new());

    // ## 跟踪词分类
    lines.push("## 跟踪词分类".into());
    if !a.tracked_term_categories.is_empty() {
        for item in &a.tracked_term_categories {
            lines.push(format!(
                "- `{}` `{}` count=`{}` active_terms=`{}` warn_terms=`{}`",
                if item.warn { "WARN" } else { "OK" },
                item.category,
                item.count,
                item.active_terms,
                item.warn_terms
            ));
            for term in item.top_terms.iter().take(5) {
                lines.push(format!(
                    "  - `{}` x{} per_10k=`{}` warn=`{}`",
                    term.term,
                    term.count,
                    float_repr(term.per_10k),
                    if term.warn { "Y" } else { "N" }
                ));
            }
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    // ## 点名局部密度
    lines.push("## 点名局部密度".into());
    if !a.tracked_term_windows.is_empty() {
        let shown = a.tracked_term_windows.len().min(8);
        lines.push(format!(
            "- 命中总数：`{}`；展示：`{}`",
            a.tracked_term_window_count, shown
        ));
        for item in a.tracked_term_windows.iter().take(shown) {
            lines.push(format!(
                "- `S{}-{}` `L{}-{}` score=`{}`：{}",
                item.start_index,
                item.end_index,
                item.start_line,
                item.end_line,
                item.score,
                markdown_table_cell(&item.reasons.join("、"))
            ));
            let terms = format_tracked_term_counts(&item.terms, "，");
            if !terms.is_empty() {
                lines.push(format!("  词项：{}", markdown_table_cell(&terms)));
            }
            if !item.suggestion.is_empty() {
                lines.push(format!("  建议：{}", markdown_table_cell(&item.suggestion)));
            }
            let sample = item
                .sample
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            lines.push(format!("  样例：{}", markdown_table_cell(&sample)));
        }
    } else {
        lines.push("- 无".into());
    }
    lines.push(String::new());

    // ## 高频词片段 / 结构短语（各取前 15）
    lines.push("## 高频词片段".into());
    if a.terms.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in a.terms.iter().take(15) {
            lines.push(format!("- `{}` x{}", item.term, item.count));
        }
    }
    lines.push(String::new());

    lines.push("## 结构短语".into());
    if a.short_phrases.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in a.short_phrases.iter().take(15) {
            lines.push(format!("- `{}` x{}", item.term, item.count));
        }
    }
    lines.push(String::new());

    // ## 句式骨架 / 判断句尾（全量）
    lines.push("## 句式骨架".into());
    if a.sentence_patterns.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in &a.sentence_patterns {
            lines.push(format!("- `{}` x{}", item.phrase, item.count));
        }
    }
    lines.push(String::new());

    lines.push("## 判断句尾".into());
    if a.judgement_endings.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in &a.judgement_endings {
            lines.push(format!("- `{}` x{}", item.phrase, item.count));
        }
    }
    lines.push(String::new());

    // ## 判断句上下文
    lines.push("## 判断句上下文".into());
    if a.judgement_contexts.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in &a.judgement_contexts {
            let status = if item.warn {
                "WARN"
            } else if item.watch {
                "WATCH"
            } else {
                "OK"
            };
            let terms = if item.top_terms.is_empty() {
                "无".to_string()
            } else {
                item.top_terms
                    .iter()
                    .map(|t| format!("{} x{}", t.term, t.count))
                    .collect::<Vec<_>>()
                    .join("，")
            };
            lines.push(format!(
                "- `{status}` `{}` count=`{}` terms={terms}",
                item.label, item.count
            ));
            for sample in item.samples.iter().take(5) {
                lines.push(format!(
                    "  - `S{}` `L{}` `{}`：{}",
                    sample.index,
                    sample.line_no,
                    sample.terms.join(","),
                    sample.text
                ));
            }
        }
    }
    lines.push(String::new());

    // ## 句首重复 / 主语起手 / 段首起手（全量）
    for (header, items) in [
        ("句首重复", &a.sentence_starts),
        ("主语起手", &a.subject_leads),
        ("段首起手", &a.paragraph_leads),
    ] {
        lines.push(format!("## {header}"));
        if items.is_empty() {
            lines.push("- 无".into());
        } else {
            for item in items {
                lines.push(format!("- `{}` x{}", item.phrase, item.count));
            }
        }
        lines.push(String::new());
    }

    // ## 分句骨架 / 并列分句（各取前 15）
    for (header, items) in [
        ("分句骨架", &a.clause_prefixes),
        ("并列分句", &a.parallel_clauses),
    ] {
        lines.push(format!("## {header}"));
        if items.is_empty() {
            lines.push("- 无".into());
        } else {
            for item in items.iter().take(15) {
                lines.push(format!("- `{}` x{}", item.phrase, item.count));
            }
        }
        lines.push(String::new());
    }

    // ## AA/BB 式短节奏（取前 15）
    lines.push("## AA/BB 式短节奏".into());
    if a.aa_bb_patterns.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in a.aa_bb_patterns.iter().take(15) {
            lines.push(format!(
                "- `{}` `{}` `{}` x{}：{}",
                if item.warn { "WARN" } else { "OK" },
                item.pattern_type,
                item.name,
                item.count,
                item.note
            ));
            if let Some(first) = item.samples.first() {
                lines.push(format!("  样例：{first}"));
            }
        }
    }
    lines.push(String::new());

    // ## 逐句字数
    lines.push("## 逐句字数".into());
    let sl = &a.sentence_lengths;
    lines.push(format!(
        "- 状态：`{}` count=`{}` min=`{}` p10=`{}` p25=`{}` median=`{}` avg=`{}` max=`{}`",
        if sl.warn { "WARN" } else { "OK" },
        sl.count,
        sl.min_chars,
        sl.p10_chars,
        sl.p25_chars,
        sl.median_chars,
        float_repr(sl.avg_chars),
        sl.max_chars
    ));
    lines.push(format!(
        "- 短句：`{}`；极短句：`{}`；短句占比：`{}`；短句连发：`{}`",
        sl.short_count,
        sl.very_short_count,
        float_repr(sl.short_ratio),
        sl.short_runs.len()
    ));
    if !sl.short_sentences.is_empty() {
        for item in sl.short_sentences.iter().take(12) {
            lines.push(format!(
                "- `S{}` `L{}` `{}字`：{}",
                item.index, item.line_no, item.chars, item.text
            ));
        }
    }
    if !sl.short_runs.is_empty() {
        lines.push("- 短句连发样例：".into());
        for item in sl.short_runs.iter().take(5) {
            let roles = format_short_roles(&item.roles, "，");
            lines.push(format!(
                "- `S{}-{}` `L{}-{}` avg=`{}`：{}",
                item.start_index,
                item.end_index,
                item.start_line,
                item.end_line,
                float_repr(item.avg_chars),
                item.sample.join(" | ")
            ));
            if !roles.is_empty() {
                lines.push(format!("  类型：{roles}"));
            }
            if !item.suggestion.is_empty() {
                lines.push(format!("  建议：{}", item.suggestion));
            }
        }
    }
    if !a.source.contains(" | ") {
        lines.push(String::new());
        lines.push("### 每句字数明细".into());
        for item in &sl.sentences {
            lines.push(format!(
                "- `S{}` `L{}` `{}字`：{}",
                item.index, item.line_no, item.chars, item.text
            ));
        }
    }
    lines.push(String::new());

    // ## 对话
    lines.push("## 对话".into());
    let d = &a.dialogue;
    lines.push(format!(
        "- 连续短对白块：`{}`",
        d.consecutive_quote_paragraph_runs.len()
    ));
    for item in d.consecutive_quote_paragraph_runs.iter().take(5) {
        lines.push(format!(
            "- 段落 `{}-{}`: {}",
            item.start_paragraph,
            item.end_paragraph,
            item.sample.join(" | ")
        ));
    }
    lines.push(format!("- 短句对白块：`{}`", d.short_quote_runs.len()));
    for item in d.short_quote_runs.iter().take(5) {
        lines.push(format!(
            "- 段落 `{}-{}` 平均句长=`{}`: {}",
            item.start_paragraph,
            item.end_paragraph,
            float_repr(item.avg_len),
            item.sample.join(" | ")
        ));
    }
    lines.push(format!("- 问答互顶块：`{}`", d.question_ping_pong.len()));
    for item in d.question_ping_pong.iter().take(5) {
        lines.push(format!(
            "- 段落 `{}-{}`: {}",
            item.start_paragraph,
            item.end_paragraph,
            item.sample.join(" | ")
        ));
    }
    lines.push(format!("- 白话乒乓块：`{}`", d.quote_ping_pong.len()));
    for item in d.quote_ping_pong.iter().take(5) {
        lines.push(format!(
            "- 段落 `{}-{}` 平均句长=`{}`: {}",
            item.start_paragraph,
            item.end_paragraph,
            float_repr(item.avg_len),
            item.sample.join(" | ")
        ));
    }
    lines.push(format!("- 对白转轴缺口：`{}`", d.dialogue_axis_gaps.len()));
    for item in d.dialogue_axis_gaps.iter().take(5) {
        lines.push(format!(
            "- `S{}-{}` `L{}-{}` score=`{}`：{}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.score,
            markdown_table_cell(&item.reasons.join("、"))
        ));
        lines.push(format!("  建议：{}", markdown_table_cell(&item.suggestion)));
        let sample = item
            .sample
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("  样例：{}", markdown_table_cell(&sample)));
    }
    lines.push(format!(
        "- A/B 乒乓：`{}`",
        d.alternating_speaker_runs.len()
    ));
    for item in d.alternating_speaker_runs.iter().take(5) {
        lines.push(format!("- 段落 `{}`: `{}`", item.paragraph, item.pattern));
    }
    lines.push(format!(
        "- 对话段占比：`{}`",
        float_repr(d.quote_paragraph_ratio)
    ));
    lines.push(format!("- 最长连续对白块：`{}`", d.dense_quote_run_max));
    lines.push(format!("- 超长对白块数：`{}`", d.dense_quote_run_count));
    lines.push(String::new());

    // ## 活跃标点（取前 10）
    lines.push("## 活跃标点".into());
    if a.dominant_punctuation.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in a.dominant_punctuation.iter().take(10) {
            lines.push(format!(
                "- `{}` x{} per_10k=`{}`",
                item.mark,
                item.count,
                float_repr(item.per_10k)
            ));
        }
    }
    lines.push(String::new());

    // ## 形容词 / 动词压力（只展示 total > 0 的行）
    lines.push("## 形容词 / 动词压力".into());
    let active: Vec<&ModifierPressure> = a
        .modifier_pressure
        .iter()
        .filter(|item| item.total > 0)
        .collect();
    if active.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in active {
            lines.push(format!(
                "- `{}` `{}` total=`{}` dense_sentences=`{}`",
                if item.warn { "WARN" } else { "OK" },
                item.label,
                item.total,
                item.dense_sentences
            ));
        }
    }
    lines.push(String::new());

    // ## 章末检查
    lines.push("## 章末检查".into());
    lines.push(format!(
        "- 状态：`{}`",
        if a.ending.warn { "WARN" } else { "OK" }
    ));
    if !a.ending.image_terms.is_empty() {
        lines.push(format!("- 章末意象词：`{ending_image_md}`"));
    }
    if !a.ending.flow_terms.is_empty() {
        lines.push(format!("- 章末流程词：`{ending_flow_md}`"));
    }
    let tail = prefix_chars(&a.ending.tail_excerpt, 100);
    lines.push(format!(
        "- 章末摘录：{}",
        if tail.is_empty() {
            "无"
        } else {
            tail.as_str()
        }
    ));
    lines.push(String::new());

    // ## 模版候选（取前 20）
    lines.push("## 模版候选".into());
    if a.template_candidates.is_empty() {
        lines.push("- 无".into());
    } else {
        for item in a.template_candidates.iter().take(20) {
            let mut line = format!(
                "- `{}` `{}` x{}：{}",
                item.candidate_type, item.name, item.count, item.note
            );
            if !item.sample.is_empty() {
                line.push_str(&format!("；样例：{}", item.sample));
            }
            lines.push(line);
        }
    }
    lines.push(String::new());

    lines.join("\n")
}

/// `run` 的 CLI 参数。
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// 位置参数：草稿文件或目录。
    pub positional: Vec<PathBuf>,
    /// `-i/--input`：输入文件或目录（可重复）。
    pub inputs: Vec<PathBuf>,
    /// 每条规则最多记录的样本行数（默认 3）。
    pub sample_limit: usize,
    /// `--fail-on-warn`：有警告时退出码 1。
    pub fail_on_warn: bool,
    /// `--format`：报告格式（json/text/markdown）。
    pub format: ReportFormat,
    /// `-o/--output`：输出文件。
    pub output: Option<PathBuf>,
    /// `--learn-from`：语料路径（缺省时自动定位同小说语料）。
    pub learn_from: Option<Vec<PathBuf>>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// `--no-corpus-learning`：禁用语料学习。
    pub no_corpus_learning: bool,
}

/// 多输入收集、语料学习开关、报告写出与退出码。
pub fn run(opts: &RunOptions) -> Result<i32> {
    let raw_inputs = resolve_inputs(&opts.positional, &opts.inputs)?;
    let files = iter_target_files(&raw_inputs);
    if files.is_empty() {
        eprintln!("No target files found.");
        return Ok(2);
    }
    let rules = crate::config::load_rules_from(opts.rules.as_deref())?;
    let ctx = DraftContext::new(rules)?;
    let template_bank = build_template_bank(ctx.draft_rules());
    let corpus_profile: Option<CorpusProfile> = if opts.no_corpus_learning {
        None
    } else {
        match opts.learn_from.as_ref() {
            // 显式指定语料：尊重用户选择，原样计入。
            Some(paths) => build_corpus_profile(&ctx, paths, &[])?,
            // 自动定位语料：排除本次分析目标自身，避免本章高频词
            // 被「语料学到的高频词」在本章自我实现。
            None => {
                let corpus_paths = ctx.corpus_paths_for_targets(&files);
                build_corpus_profile(&ctx, &corpus_paths, &files)?
            }
        }
    };
    let term_bank = ctx.draft_rules().tracked_terms.clone();
    let mut any_warn = false;
    let mut reports: Vec<Analysis> = Vec::new();
    for path in &files {
        let analysis = analyze_path(
            &ctx,
            path,
            &template_bank,
            &term_bank,
            corpus_profile.as_ref(),
            opts.sample_limit,
        )?;
        any_warn |= analysis.warned;
        reports.push(analysis);
    }
    if opts.format == ReportFormat::Json {
        let json = serde_json::to_string_pretty(&reports)?;
        match &opts.output {
            Some(path) => write_json_line(path, &json)?,
            None => println!("{json}"),
        }
    } else {
        let suffix = if opts.format == ReportFormat::Markdown {
            ".md"
        } else {
            ".txt"
        };
        match &opts.output {
            None => {
                for report in &reports {
                    println!("{}", render_report(report, opts.format, opts.sample_limit));
                    println!();
                }
            }
            Some(out_path) => {
                if reports.len() == 1 {
                    let rendered = render_report(&reports[0], opts.format, opts.sample_limit);
                    write_text(out_path, &format!("{rendered}\n"))?;
                } else {
                    fs::create_dir_all(out_path)
                        .with_context(|| format!("无法创建输出目录 {}", out_path.display()))?;
                    for report in &reports {
                        let stem = path_stem(&report.source);
                        let file = out_path.join(format!("{stem}{suffix}"));
                        let rendered = render_report(report, opts.format, opts.sample_limit);
                        write_text(&file, &format!("{rendered}\n"))?;
                    }
                }
            }
        }
    }
    if opts.fail_on_warn && any_warn {
        return Ok(1);
    }
    Ok(0)
}
