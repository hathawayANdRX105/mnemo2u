//! 文本报告渲染（`format_text_report` / `render_report`，输出逐字节稳定）。

use super::*;

// ---------------------------------------------------------------------------
// 文本报告

/// 文本指标行（4 类指标的渲染字段形状一致）。
struct TextMetric<'a> {
    name: &'a str,
    count: usize,
    per_10k: f64,
    max_per_10k: f64,
    note: &'a str,
    warn: bool,
    samples: &'a [Hit],
}

fn text_metric<'a>(
    name: &'a str,
    count: usize,
    per_10k: f64,
    max_per_10k: f64,
    note: &'a str,
    warn: bool,
    samples: &'a [Hit],
) -> TextMetric<'a> {
    TextMetric {
        name,
        count,
        per_10k,
        max_per_10k,
        note,
        warn,
        samples,
    }
}

/// 渲染一个规则指标小节。
fn metric_block_lines(title: &str, metrics: &[TextMetric<'_>], sample_limit: usize) -> Vec<String> {
    let mut out = vec![format!("{title}:")];
    for metric in metrics {
        let status = if metric.warn { "WARN" } else { "OK" };
        out.push(format!(
            "  [{status}] {name}: count={count}, per_10k={per_10k:.2}, max={max:.2}  # {note}",
            name = metric.name,
            count = metric.count,
            per_10k = metric.per_10k,
            max = metric.max_per_10k,
            note = metric.note,
        ));
        for sample in metric.samples.iter().take(sample_limit) {
            out.push(format!("    L{}: {}", sample.line_no, sample.snippet));
        }
    }
    out
}

/// 四类指标 → 文本行（字段形状一致；`name/count/per_10k/max_per_10k/note/warn/samples`）。
fn regex_text_rows(list: &[RegexMetric]) -> Vec<TextMetric<'_>> {
    list.iter()
        .map(|m| {
            text_metric(
                &m.name,
                m.count,
                m.per_10k,
                m.max_per_10k,
                &m.note,
                m.warn,
                &m.samples,
            )
        })
        .collect()
}

fn tracked_text_rows(list: &[crate::rules::TrackedMetric]) -> Vec<TextMetric<'_>> {
    list.iter()
        .map(|m| {
            text_metric(
                &m.name,
                m.count,
                m.per_10k,
                m.max_per_10k,
                &m.note,
                m.warn,
                &m.samples,
            )
        })
        .collect()
}

fn custom_text_rows(list: &[CustomTemplateMetric]) -> Vec<TextMetric<'_>> {
    list.iter()
        .map(|m| {
            text_metric(
                &m.name,
                m.count,
                m.per_10k,
                m.max_per_10k,
                &m.note,
                m.warn,
                &m.samples,
            )
        })
        .collect()
}

fn learned_text_rows(list: &[LearnedFilterMetric]) -> Vec<TextMetric<'_>> {
    list.iter()
        .map(|m| {
            text_metric(
                &m.name,
                m.count,
                m.per_10k,
                m.max_per_10k,
                &m.note,
                m.warn,
                &m.samples,
            )
        })
        .collect()
}

/// 把完整 analysis 渲染成文本报告。
#[must_use]
pub fn format_text_report(a: &Analysis, sample_limit: usize) -> String {
    let s = &a.summary;
    let mut output: Vec<String> = vec![
        format!("FILE {}", a.source),
        format!("chars={}", s.chars),
        format!("sentences={}", s.sentences),
        format!("paragraphs={}", s.paragraphs),
        format!("avg_sentence_chars={}", float_repr(s.avg_sentence_chars)),
        format!("short_sentences={}", s.short_sentences),
        format!("very_short_sentences={}", s.very_short_sentences),
        format!(
            "short_sentence_ratio={}",
            float_repr(s.short_sentence_ratio)
        ),
        format!("quote_ratio={}", float_repr(s.quote_ratio)),
        format!("warn_sections={}", s.warn_sections),
    ];

    // hard_flags
    output.push("hard_flags:".into());
    output.push(format!("  count={}", a.hard_flags.len()));
    for item in a.hard_flags.iter().take(sample_limit * 8) {
        let mut line = format!("    [{}] {}: count={}", item.section, item.name, item.count);
        if let Some(per_10k) = item.per_10k {
            line.push_str(&format!(", per_10k={}", float_repr(per_10k)));
        }
        line.push_str(&format!("  # {}", item.note));
        if item.register != "neutral" {
            line.push_str(&format!(" [{}]", item.register));
        }
        if !item.sample.is_empty() {
            line.push_str(&format!(" | {}", item.sample));
        }
        output.push(line);
    }

    // review_reminders
    output.push("review_reminders:".into());
    output.push(format!("  count={}", a.review_reminders.len()));
    for item in a.review_reminders.iter().take(sample_limit * 4) {
        output.push(format!(
            "    [{}] {} {}  # {}",
            item.priority, item.category, item.title, item.reason
        ));
        output.push(format!("      check: {}", item.check));
        output.push(format!("      action: {}", item.action));
        for evidence in item.evidence.iter().take(sample_limit) {
            output.push(format!("      evidence: {evidence}"));
        }
    }

    // style_fatigue
    output.push("style_fatigue:".into());
    output.push(format!("  count={}", a.style_fatigue.len()));
    for item in &a.style_fatigue {
        output.push(format!(
            "    [{}] {}: count={}  # {}",
            item.status, item.family, item.count, item.risk
        ));
        output.push(format!("      reduce: {}", item.reduce));
        for evidence in item.evidence.iter().take(sample_limit) {
            output.push(format!("      evidence: {evidence}"));
        }
    }

    // fatigue_windows
    output.push("fatigue_windows:".into());
    output.push(format!(
        "  count={}, shown={}",
        a.fatigue_window_count,
        a.fatigue_windows.len()
    ));
    for item in a.fatigue_windows.iter().take(sample_limit) {
        let roles = format_short_roles(&item.roles, ",");
        output.push(format!(
            "    S{}-{} L{}-{} score={} reasons={} roles={}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.score,
            item.reasons.join(","),
            roles
        ));
        output.push(format!("      suggestion: {}", item.suggestion));
        let sample = item
            .sample
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        output.push(format!("      sample: {sample}"));
    }

    // ba_operation_contexts
    output.push("ba_operation_contexts:".into());
    output.push(format!(
        "  [{}] types={}",
        if a.ba_operation_contexts.iter().any(|c| c.warn) {
            "WARN"
        } else {
            "OK"
        },
        a.ba_operation_contexts.len()
    ));
    for item in a.ba_operation_contexts.iter().take(sample_limit * 3) {
        output.push(format!(
            "    {} {}: count={} suggestion={}",
            if item.warn { "WARN" } else { "WATCH" },
            item.role,
            item.count,
            item.suggestion
        ));
        for sample in item.samples.iter().take(sample_limit) {
            output.push(format!(
                "      - S{} L{} {}: {}",
                sample.index, sample.line_no, sample.snippet, sample.sentence
            ));
        }
    }

    // 9 个规则指标小节（顺序固定）
    output.extend(metric_block_lines(
        "tokens",
        &regex_text_rows(&a.tokens),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "tracked_terms",
        &tracked_text_rows(&a.tracked_terms),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "patterns",
        &regex_text_rows(&a.patterns),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "phrases",
        &regex_text_rows(&a.phrases),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "modifiers",
        &regex_text_rows(&a.modifiers),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "punctuation",
        &regex_text_rows(&a.punctuation),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "punctuation_combos",
        &regex_text_rows(&a.punctuation_combos),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "custom_templates",
        &custom_text_rows(&a.custom_templates),
        sample_limit,
    ));
    output.extend(metric_block_lines(
        "learned_filters",
        &learned_text_rows(&a.learned_filters),
        sample_limit,
    ));

    // corpus_profile
    let p = &a.corpus_profile;
    output.push("corpus_profile:".into());
    output.push(format!(
        "  [{}] sources={} chars={} draft_chars={}",
        if p.enabled { "OK" } else { "OFF" },
        p.source_count,
        p.chars,
        p.draft_chars
    ));
    if let BaselineJson::Values(baseline) = &p.sentence_length_baseline {
        output.push(format!(
            "  baseline_sentence_chars: p10={}, p25={}, median={}, avg={}, short_ratio={}",
            baseline.p10_chars,
            baseline.p25_chars,
            baseline.median_chars,
            float_repr(baseline.avg_chars),
            float_repr(baseline.short_ratio)
        ));
    }
    for item in p.learned_sentence_leads.iter().take(sample_limit) {
        output.push(format!(
            "    learned_lead {}: count={}, corpus_per_10k={}",
            item.phrase,
            item.count,
            float_repr(item.corpus_per_10k)
        ));
    }
    for item in p.learned_aa_bb_shapes.iter().take(sample_limit) {
        output.push(format!(
            "    learned_aa_bb {}: count={}",
            item.name, item.count
        ));
    }

    // tracked_term_categories
    output.push("tracked_term_categories:".into());
    output.push(format!(
        "  [{}] active_categories={}",
        if a.tracked_term_categories.iter().any(|c| c.warn) {
            "WARN"
        } else {
            "OK"
        },
        a.tracked_term_categories.len()
    ));
    for item in a.tracked_term_categories.iter().take(sample_limit * 4) {
        output.push(format!(
            "    {}: count={}, active_terms={}, warn_terms={}",
            item.category, item.count, item.active_terms, item.warn_terms
        ));
        for term in item.top_terms.iter().take(3) {
            output.push(format!(
                "      - {}: count={}, per_10k={}, warn={}",
                term.term,
                term.count,
                float_repr(term.per_10k),
                if term.warn { "Y" } else { "N" }
            ));
        }
    }

    // tracked_term_windows
    output.push("tracked_term_windows:".into());
    output.push(format!(
        "  count={}, shown={}",
        a.tracked_term_window_count,
        a.tracked_term_windows.len()
    ));
    for item in a.tracked_term_windows.iter().take(sample_limit) {
        let terms = format_tracked_term_counts(&item.terms, ",");
        output.push(format!(
            "    S{}-{} L{}-{} score={} reasons={} terms={}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.score,
            item.reasons.join(","),
            terms
        ));
        output.push(format!("      suggestion: {}", item.suggestion));
        let sample = item
            .sample
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        output.push(format!("      sample: {sample}"));
    }

    // 短语计数小节（标题 + `[WARN|OK] key=N` + `短语: 计数` 行）
    let phrase_block =
        |output: &mut Vec<String>, title: &str, key: &str, items: &[PhraseCount], limit: usize| {
            output.push(format!("{title}:"));
            output.push(format!(
                "  [{}] {key}={}",
                if items.is_empty() { "OK" } else { "WARN" },
                items.len()
            ));
            for item in items.iter().take(limit) {
                output.push(format!("    {}: {}", item.phrase, item.count));
            }
        };
    phrase_block(
        &mut output,
        "sentence_starts",
        "repeated_sentence_leads",
        &a.sentence_starts,
        sample_limit * 3,
    );
    phrase_block(
        &mut output,
        "subject_leads",
        "repeated_subject_leads",
        &a.subject_leads,
        sample_limit * 4,
    );
    phrase_block(
        &mut output,
        "paragraph_leads",
        "repeated_paragraph_leads",
        &a.paragraph_leads,
        sample_limit * 4,
    );
    phrase_block(
        &mut output,
        "sentence_patterns",
        "repeated_sentence_skeletons",
        &a.sentence_patterns,
        sample_limit * 4,
    );
    phrase_block(
        &mut output,
        "judgement_endings",
        "repeated_judgement_endings",
        &a.judgement_endings,
        sample_limit * 4,
    );

    // judgement_contexts
    output.push("judgement_contexts:".into());
    output.push(format!(
        "  [{}] contexts={}",
        if a.judgement_contexts.iter().any(|c| c.warn) {
            "WARN"
        } else {
            "OK"
        },
        a.judgement_contexts.len()
    ));
    for item in &a.judgement_contexts {
        let terms = if item.top_terms.is_empty() {
            "无".to_string()
        } else {
            item.top_terms
                .iter()
                .map(|t| format!("{}:{}", t.term, t.count))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let status = if item.warn {
            "WARN"
        } else if item.watch {
            "WATCH"
        } else {
            "OK"
        };
        output.push(format!(
            "    {status} {}: count={} terms={}",
            item.label, item.count, terms
        ));
        for sample in item.samples.iter().take(sample_limit) {
            output.push(format!(
                "      S{} L{} {}: {}",
                sample.index,
                sample.line_no,
                sample.terms.join(","),
                sample.text
            ));
        }
    }

    phrase_block(
        &mut output,
        "clause_prefixes",
        "repeated_clause_prefixes",
        &a.clause_prefixes,
        sample_limit * 4,
    );
    phrase_block(
        &mut output,
        "parallel_clauses",
        "repeated_parallel_clauses",
        &a.parallel_clauses,
        sample_limit * 4,
    );

    // aa_bb_patterns
    output.push("aa_bb_patterns:".into());
    output.push(format!(
        "  [{}] aa_bb_patterns={}",
        if a.aa_bb_patterns.iter().any(|p| p.warn) {
            "WARN"
        } else {
            "OK"
        },
        a.aa_bb_patterns.len()
    ));
    for item in a.aa_bb_patterns.iter().take(sample_limit * 4) {
        output.push(format!(
            "    {} {} {}: count={}  # {}",
            if item.warn { "WARN" } else { "OK" },
            item.pattern_type,
            item.name,
            item.count,
            item.note
        ));
        for sample in item.samples.iter().take(sample_limit) {
            output.push(format!("      - {sample}"));
        }
    }

    // sentence_lengths
    let sl = &a.sentence_lengths;
    output.push("sentence_lengths:".into());
    output.push(format!(
        "  [{}] count={}, min={}, p10={}, p25={}, median={}, avg={}, max={}",
        if sl.warn { "WARN" } else { "OK" },
        sl.count,
        sl.min_chars,
        sl.p10_chars,
        sl.p25_chars,
        sl.median_chars,
        float_repr(sl.avg_chars),
        sl.max_chars
    ));
    output.push(format!(
        "    short_count={}, very_short_count={}, short_ratio={}, short_runs={}",
        sl.short_count,
        sl.very_short_count,
        float_repr(sl.short_ratio),
        sl.short_runs.len()
    ));
    for item in sl.short_sentences.iter().take(sample_limit * 4) {
        output.push(format!(
            "    S{} L{} chars={}: {}",
            item.index, item.line_no, item.chars, item.text
        ));
    }
    for item in sl.short_runs.iter().take(sample_limit) {
        let roles = format_short_roles(&item.roles, ",");
        output.push(format!(
            "    run S{}-{} L{}-{} avg={}: {}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            float_repr(item.avg_chars),
            item.sample.join(" | ")
        ));
        if !roles.is_empty() {
            output.push(format!("      roles: {roles}"));
        }
        if !item.suggestion.is_empty() {
            output.push(format!("      suggestion: {}", item.suggestion));
        }
    }

    // terms / short_phrases
    output.push("terms:".into());
    output.push(format!(
        "  [{}] repeated_terms={}",
        if a.terms.is_empty() { "OK" } else { "WARN" },
        a.terms.len()
    ));
    for item in a.terms.iter().take(sample_limit * 5) {
        output.push(format!("    {}: {}", item.term, item.count));
    }
    output.push("short_phrases:".into());
    output.push(format!(
        "  [{}] repeated_short_phrases={}",
        if a.short_phrases.is_empty() {
            "OK"
        } else {
            "WARN"
        },
        a.short_phrases.len()
    ));
    for item in a.short_phrases.iter().take(sample_limit * 5) {
        output.push(format!("    {}: {}", item.term, item.count));
    }

    // dialogue
    let d = &a.dialogue;
    output.push("dialogue:".into());
    let runs = &d.consecutive_quote_paragraph_runs;
    output.push(format!(
        "  [{}] consecutive_quote_paragraph_runs={}",
        if runs.is_empty() { "OK" } else { "WARN" },
        runs.len()
    ));
    for item in runs.iter().take(sample_limit) {
        output.push(format!(
            "    paragraph {}-{}: {}",
            item.start_paragraph,
            item.end_paragraph,
            item.sample.join(" | ")
        ));
    }
    let short_runs = &d.short_quote_runs;
    output.push(format!(
        "  [{}] short_quote_runs={}",
        if short_runs.is_empty() { "OK" } else { "WARN" },
        short_runs.len()
    ));
    for item in short_runs.iter().take(sample_limit) {
        output.push(format!(
            "    paragraph {}-{}: avg_len={} | {}",
            item.start_paragraph,
            item.end_paragraph,
            float_repr(item.avg_len),
            item.sample.join(" | ")
        ));
    }
    let question_runs = &d.question_ping_pong;
    output.push(format!(
        "  [{}] question_ping_pong={}",
        if question_runs.is_empty() {
            "OK"
        } else {
            "WARN"
        },
        question_runs.len()
    ));
    for item in question_runs.iter().take(sample_limit) {
        output.push(format!(
            "    paragraph {}-{}: {}",
            item.start_paragraph,
            item.end_paragraph,
            item.sample.join(" | ")
        ));
    }
    let ping_pong_runs = &d.quote_ping_pong;
    output.push(format!(
        "  [{}] quote_ping_pong={}",
        if ping_pong_runs.is_empty() {
            "OK"
        } else {
            "WARN"
        },
        ping_pong_runs.len()
    ));
    for item in ping_pong_runs.iter().take(sample_limit) {
        output.push(format!(
            "    paragraph {}-{}: avg_len={} | {}",
            item.start_paragraph,
            item.end_paragraph,
            float_repr(item.avg_len),
            item.sample.join(" | ")
        ));
    }
    let axis_gaps = &d.dialogue_axis_gaps;
    output.push(format!(
        "  [{}] dialogue_axis_gaps={}",
        if axis_gaps.is_empty() { "OK" } else { "WARN" },
        axis_gaps.len()
    ));
    for item in axis_gaps.iter().take(sample_limit) {
        output.push(format!(
            "    S{}-{} L{}-{} score={} reasons={}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.score,
            item.reasons.join(",")
        ));
        output.push(format!("      suggestion: {}", item.suggestion));
        let sample = item
            .sample
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        output.push(format!("      sample: {sample}"));
    }
    let turns = &d.alternating_speaker_runs;
    output.push(format!(
        "  [{}] alternating_speaker_runs={}",
        if turns.is_empty() { "OK" } else { "WARN" },
        turns.len()
    ));
    for item in turns.iter().take(sample_limit) {
        output.push(format!(
            "    paragraph {}: {}",
            item.paragraph, item.pattern
        ));
    }
    output.push(format!(
        "  [INFO] quote_paragraph_ratio={}",
        float_repr(d.quote_paragraph_ratio)
    ));
    output.push(format!(
        "  [INFO] dense_quote_run_max={}",
        d.dense_quote_run_max
    ));
    output.push(format!(
        "  [INFO] dense_quote_run_count={}",
        d.dense_quote_run_count
    ));

    // dominant_punctuation
    output.push("dominant_punctuation:".into());
    output.push(format!(
        "  [{}] active_marks={}",
        if a.dominant_punctuation.is_empty() {
            "OK"
        } else {
            "WARN"
        },
        a.dominant_punctuation.len()
    ));
    for item in a.dominant_punctuation.iter().take(sample_limit * 4) {
        output.push(format!(
            "    {}: count={}, per_10k={}",
            item.mark,
            item.count,
            float_repr(item.per_10k)
        ));
    }

    // modifier_pressure（只展示 total > 0 的行）
    output.push("modifier_pressure:".into());
    let active: Vec<&ModifierPressure> = a
        .modifier_pressure
        .iter()
        .filter(|item| item.total > 0)
        .collect();
    output.push(format!(
        "  [{}] active_groups={}",
        if active.iter().any(|item| item.warn) {
            "WARN"
        } else {
            "OK"
        },
        active.len()
    ));
    for item in active.iter().take(sample_limit) {
        output.push(format!(
            "    {}: total={}, dense_sentences={}, warn={}",
            item.label,
            item.total,
            item.dense_sentences,
            if item.warn { "Y" } else { "N" }
        ));
    }

    // ending
    output.push("ending:".into());
    output.push(format!(
        "  [{}] tail_template_check",
        if a.ending.warn { "WARN" } else { "OK" }
    ));
    let image_summary = if a.ending.image_terms.is_empty() {
        "无".to_string()
    } else {
        a.ending
            .image_terms
            .iter()
            .map(|t| format!("{}:{}", t.term, t.count))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let flow_summary = if a.ending.flow_terms.is_empty() {
        "无".to_string()
    } else {
        a.ending
            .flow_terms
            .iter()
            .map(|t| format!("{}:{}", t.term, t.count))
            .collect::<Vec<_>>()
            .join(", ")
    };
    output.push(format!("    image_terms={image_summary}"));
    output.push(format!("    flow_terms={flow_summary}"));

    // template_candidates
    output.push("template_candidates:".into());
    output.push(format!("  count={}", a.template_candidates.len()));
    for item in a.template_candidates.iter().take(sample_limit * 6) {
        let mut line = format!(
            "    [{}] {} x{}  # {}",
            item.candidate_type, item.name, item.count, item.note
        );
        if !item.sample.is_empty() {
            line.push_str(&format!(" | {}", item.sample));
        }
        output.push(line);
    }

    output.join("\n")
}

/// source 文件名 stem：markdown 报告标题与多文件输出命名。
pub(crate) fn path_stem(source: &str) -> String {
    Path::new(source)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 报告分发：markdown 标题取 source 的 stem
/// （空时回退 source），text 走 sample_limit；JSON 由调用方直接序列化。
#[must_use]
pub fn render_report(a: &Analysis, format: ReportFormat, sample_limit: usize) -> String {
    debug_assert!(
        format != ReportFormat::Json,
        "JSON 分支由调用方 serde 序列化，不走渲染"
    );
    if format == ReportFormat::Markdown {
        let title = path_stem(&a.source);
        return format_markdown_report(a, if title.is_empty() { None } else { Some(&title) });
    }
    format_text_report(a, sample_limit)
}
