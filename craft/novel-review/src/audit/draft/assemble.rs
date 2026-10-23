//! `analyze_text` 尾部装配：`AnalysisSections` → `template_candidates`/`hard_flags`/`summary` → `Analysis`。

use super::*;

/// `analyze_text` 尾部：装配 `template_candidates`/`hard_flags`/`summary`/`Analysis`
/// （体与原 `analyze_text` 逐行一致）。
pub(crate) fn assemble_analysis(sections: AnalysisSections<'_>, source: &str) -> Result<Analysis> {
    let AnalysisSections {
        text,
        corpus_profile,
        chars,
        sentence_infos,
        paragraphs,
        token_metrics,
        pattern_metrics,
        phrase_metrics,
        modifier_metrics,
        punctuation_metrics,
        combo_metrics,
        token_warn,
        pattern_warn,
        phrase_warn,
        modifier_warn,
        punctuation_warn,
        combo_warn,
        custom_template_metrics,
        custom_warn,
        tracked_term_metrics,
        tracked_term_categories,
        tracked_term_warn,
        learned_filter_metrics,
        learned_filter_warn,
        repeated_starts,
        subject_leads_rows,
        paragraph_leads_rows,
        sentence_patterns,
        judgement_endings,
        clause_prefixes,
        parallel_clauses,
        aa_bb_patterns,
        aa_bb_warn,
        ba_operation_contexts,
        ba_operation_context_warn,
        modifier_pressure,
        sentence_lengths,
        fatigue_windows,
        fatigue_window_count,
        judgement_contexts,
        judgement_context_warn,
        dominant_punctuation,
        terms,
        short_phrases,
        dialogue,
        dialogue_warn,
        scene_map,
        dialogue_emotions,
        character_voice,
        tone_profile,
        battle_profile,
        viewpoint_profile,
        ending,
        ending_warn,
        tracked_term_windows,
        tracked_term_window_count,
        template_candidates,
    } = sections;

    // hard_flags（顺序固定：先 9 指标节，再其余固定行，最后整体排序）。
    let mut hard_flags: Vec<HardFlag> = Vec::new();
    let extend_flags = |section: &str, metrics: &[RegexMetric], flags: &mut Vec<HardFlag>| {
        for m in metrics {
            if !m.warn {
                continue;
            }
            flags.push(HardFlag {
                section: section.to_string(),
                name: m.name.clone(),
                count: m.count,
                per_10k: Some(m.per_10k),
                note: m.note.clone(),
                register: m.register.clone(),
                sample: first_snippet(&m.samples),
            });
        }
    };
    for m in &tracked_term_metrics {
        if m.warn {
            hard_flags.push(HardFlag {
                section: "tracked_terms".to_string(),
                name: m.name.clone(),
                count: m.count,
                per_10k: Some(m.per_10k),
                note: m.note.clone(),
                register: "neutral".to_string(),
                sample: first_snippet(&m.samples),
            });
        }
    }
    extend_flags("tokens", &token_metrics, &mut hard_flags);
    extend_flags("patterns", &pattern_metrics, &mut hard_flags);
    extend_flags("phrases", &phrase_metrics, &mut hard_flags);
    extend_flags("modifiers", &modifier_metrics, &mut hard_flags);
    extend_flags("punctuation", &punctuation_metrics, &mut hard_flags);
    extend_flags("punctuation_combos", &combo_metrics, &mut hard_flags);
    for m in &custom_template_metrics {
        if m.warn {
            hard_flags.push(HardFlag {
                section: "custom_templates".to_string(),
                name: m.name.clone(),
                count: m.count,
                per_10k: Some(m.per_10k),
                note: m.note.clone(),
                register: "neutral".to_string(),
                sample: first_snippet(&m.samples),
            });
        }
    }
    for m in &learned_filter_metrics {
        if m.warn {
            hard_flags.push(HardFlag {
                section: "learned_filters".to_string(),
                name: m.name.clone(),
                count: m.count,
                per_10k: Some(m.per_10k),
                note: m.note.clone(),
                register: "neutral".to_string(),
                sample: first_snippet(&m.samples),
            });
        }
    }
    let push_flag = |section: &str,
                     name: String,
                     count: usize,
                     note: String,
                     sample: String,
                     flags: &mut Vec<HardFlag>| {
        flags.push(HardFlag {
            section: section.to_string(),
            name,
            count,
            per_10k: None,
            note,
            register: "neutral".to_string(),
            sample,
        });
    };
    for item in &sentence_patterns {
        push_flag(
            "sentence_patterns",
            item.phrase.clone(),
            item.count,
            "句首骨架重复".to_string(),
            String::new(),
            &mut hard_flags,
        );
    }
    for item in &subject_leads_rows {
        push_flag(
            "subject_leads",
            item.phrase.clone(),
            item.count,
            "主语起手重复".to_string(),
            String::new(),
            &mut hard_flags,
        );
    }
    for item in &paragraph_leads_rows {
        push_flag(
            "paragraph_leads",
            item.phrase.clone(),
            item.count,
            "段首起手重复".to_string(),
            String::new(),
            &mut hard_flags,
        );
    }
    for item in &clause_prefixes {
        push_flag(
            "clause_prefixes",
            item.phrase.clone(),
            item.count,
            "分句骨架重复".to_string(),
            String::new(),
            &mut hard_flags,
        );
    }
    for item in short_phrases.iter().take(10) {
        push_flag(
            "short_phrases",
            item.term.clone(),
            item.count,
            "结构短语重复".to_string(),
            String::new(),
            &mut hard_flags,
        );
    }
    for item in &aa_bb_patterns {
        if !item.warn {
            continue;
        }
        push_flag(
            "aa_bb_patterns",
            item.name.clone(),
            item.count,
            item.note.clone(),
            item.samples.first().cloned().unwrap_or_default(),
            &mut hard_flags,
        );
    }
    for item in &ba_operation_contexts {
        if !item.warn {
            continue;
        }
        let sample = item
            .samples
            .first()
            .map(|s| s.sentence.clone())
            .unwrap_or_default();
        push_flag(
            "ba_operation_contexts",
            item.role.clone(),
            item.count,
            format!("把字句类型偏密；{}", item.suggestion),
            sample,
            &mut hard_flags,
        );
    }
    if sentence_lengths.warn {
        if let Some(first_run) = sentence_lengths.short_runs.first() {
            push_flag(
                "sentence_lengths",
                "短句连发".to_string(),
                sentence_lengths.short_runs.len(),
                format!(
                    "连续短句会把叙述切成机械节拍；类型：{}；建议：{}",
                    format_short_roles(&first_run.roles, "，"),
                    first_run.suggestion
                ),
                first_run.sample.join(" | "),
                &mut hard_flags,
            );
        }
        if let Some(first_short) = sentence_lengths.short_sentences.first() {
            push_flag(
                "sentence_lengths",
                "短句密度".to_string(),
                sentence_lengths.short_count,
                "短句过多时需要判断是节奏控制还是内容没写开".to_string(),
                format!(
                    "L{} {}字：{}",
                    first_short.line_no, first_short.chars, first_short.text
                ),
                &mut hard_flags,
            );
        }
    }
    if let Some(first) = dialogue.consecutive_quote_paragraph_runs.first() {
        push_flag(
            "dialogue",
            "连续短对白块".to_string(),
            dialogue.consecutive_quote_paragraph_runs.len(),
            "对话过长且缺少动作转轴".to_string(),
            first.sample.join(" | "),
            &mut hard_flags,
        );
    }
    if let Some(first) = dialogue.short_quote_runs.first() {
        push_flag(
            "dialogue",
            "短句对白块".to_string(),
            dialogue.short_quote_runs.len(),
            "对白像互答录音".to_string(),
            first.sample.join(" | "),
            &mut hard_flags,
        );
    }
    if let Some(first) = dialogue.question_ping_pong.first() {
        push_flag(
            "dialogue",
            "问答互顶".to_string(),
            dialogue.question_ping_pong.len(),
            "问一句顶一句，像脚本对白".to_string(),
            first.sample.join(" | "),
            &mut hard_flags,
        );
    }
    if let Some(first) = dialogue.quote_ping_pong.first() {
        push_flag(
            "dialogue",
            "对白乒乓".to_string(),
            dialogue.quote_ping_pong.len(),
            "纯对白来回互顶".to_string(),
            first.sample.join(" | "),
            &mut hard_flags,
        );
    }
    if let Some(first_gap) = dialogue.dialogue_axis_gaps.first() {
        push_flag(
            "dialogue_axis_gaps",
            "对白转轴缺口".to_string(),
            dialogue.dialogue_axis_gaps.len(),
            format!(
                "连续对白缺少动作、环境、第三方或设备转轴；{}",
                first_gap.suggestion
            ),
            first_gap
                .sample
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
            &mut hard_flags,
        );
    }
    if scene_map.warn {
        let first_block = scene_map.blocks.first();
        push_flag(
            "scene_map",
            "场面功能失衡".to_string(),
            scene_map.block_count,
            format!(
                "粗分块里 `{}` 占比 `{}`，场面功能切换偏少。",
                scene_map.dominant_role,
                float_repr(scene_map.dominance_ratio)
            ),
            first_block
                .map(|b| {
                    b.sample
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default(),
            &mut hard_flags,
        );
    }
    if dialogue_emotions.flatness_warn || dialogue_emotions.volatility_warn {
        let first_sample = dialogue_emotions.samples.first();
        push_flag(
            "dialogue_emotions",
            "对白情绪单一/横跳".to_string(),
            if dialogue_emotions.shift_count > 0 {
                dialogue_emotions.shift_count
            } else {
                dialogue_emotions.dialogue_sentences
            },
            format!(
                "dominant={} ratio={} shift={}",
                dialogue_emotions.dominant_emotion,
                float_repr(dialogue_emotions.dominant_ratio),
                dialogue_emotions.shift_count
            ),
            first_sample.map(|s| s.text.clone()).unwrap_or_default(),
            &mut hard_flags,
        );
    }
    if character_voice.warn {
        push_flag(
            "character_voice",
            "角色对白同质化".to_string(),
            character_voice.speaker_count,
            format!(
                "dominant={} coverage={}，多名角色对白画像过近。",
                character_voice.dominant_speaker,
                float_repr(character_voice.coverage_ratio)
            ),
            character_voice
                .homogenized_pairs
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
            &mut hard_flags,
        );
    }
    if battle_profile.warn {
        let first_sample = battle_profile.samples.first();
        push_flag(
            "battle_profile",
            "动作链缺结果".to_string(),
            battle_profile.sequence_count,
            format!(
                "result_ratio={}，动作句已有堆积，但结果/伤害反馈不足。",
                float_repr(battle_profile.result_ratio)
            ),
            first_sample
                .map(|s| {
                    s.sample
                        .iter()
                        .take(4)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default(),
            &mut hard_flags,
        );
    }
    if viewpoint_profile.warn {
        let first_overlap = viewpoint_profile.overlaps.first();
        push_flag(
            "viewpoint_profile",
            "视角锚点漂移".to_string(),
            if viewpoint_profile.overlap_count > 0 {
                viewpoint_profile.overlap_count
            } else {
                viewpoint_profile.switch_count
            },
            "同段多人物心理暴露或近距离切锚偏多。".to_string(),
            first_overlap.map(|o| o.text.clone()).unwrap_or_default(),
            &mut hard_flags,
        );
    }
    if ending.warn {
        let sample_source = ending.tail_excerpt.replace('\n', " ");
        push_flag(
            "ending",
            "章末模板".to_string(),
            ending.image_terms.len() + ending.flow_terms.len(),
            "章末意象或流程词偏密".to_string(),
            prefix_chars(sample_source.as_str(), 80),
            &mut hard_flags,
        );
    }
    if let Some(first_window) = fatigue_windows.first() {
        push_flag(
            "fatigue_windows",
            "局部疲劳窗口".to_string(),
            fatigue_windows.len(),
            format!(
                "短句、判断、把字句、角色起手等问题在局部连续叠加；类型：{}；建议：{}",
                format_short_roles(&first_window.roles, "，"),
                first_window.suggestion
            ),
            first_window
                .sample
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
            &mut hard_flags,
        );
    }
    if let Some(first_window) = tracked_term_windows.first() {
        push_flag(
            "tracked_term_windows",
            "点名局部密度".to_string(),
            tracked_term_windows.len(),
            format!(
                "同一名词或同类跟踪词在局部窗口内密集出现；{}；建议：{}",
                format_tracked_term_counts(
                    &first_window.terms[..first_window.terms.len().min(3)],
                    "，"
                ),
                first_window.suggestion
            ),
            first_window
                .sample
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
            &mut hard_flags,
        );
    }
    for item in &judgement_contexts {
        if !item.warn {
            continue;
        }
        let sample = item
            .samples
            .first()
            .map(|s| format!("L{} {}", s.line_no, s.text))
            .unwrap_or_default();
        push_flag(
            "judgement_contexts",
            item.label.clone(),
            item.count,
            "旁白判断句偏密，容易替场面下结论".to_string(),
            sample,
            &mut hard_flags,
        );
    }
    hard_flags.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.section.cmp(&b.section))
            .then_with(|| a.name.cmp(&b.name))
    });

    let warned = token_warn
        || pattern_warn
        || phrase_warn
        || modifier_warn
        || punctuation_warn
        || combo_warn
        || custom_warn
        || tracked_term_warn
        || learned_filter_warn
        || !repeated_starts.is_empty()
        || !subject_leads_rows.is_empty()
        || !paragraph_leads_rows.is_empty()
        || !sentence_patterns.is_empty()
        || !judgement_endings.is_empty()
        || !clause_prefixes.is_empty()
        || !parallel_clauses.is_empty()
        || aa_bb_warn
        || ba_operation_context_warn
        || sentence_lengths.warn
        || modifier_pressure.iter().any(|m| m.warn)
        || !short_phrases.is_empty()
        || dialogue_warn
        || scene_map.warn
        || dialogue_emotions.flatness_warn
        || dialogue_emotions.volatility_warn
        || battle_profile.warn
        || viewpoint_profile.warn
        || !dialogue.short_quote_runs.is_empty()
        || !dialogue.question_ping_pong.is_empty()
        || !dialogue.quote_ping_pong.is_empty()
        || ending_warn
        || !fatigue_windows.is_empty()
        || judgement_context_warn
        || !tracked_term_windows.is_empty();
    let warn_sections = usize::from(token_warn)
        + usize::from(pattern_warn)
        + usize::from(phrase_warn)
        + usize::from(modifier_warn)
        + usize::from(punctuation_warn)
        + usize::from(combo_warn)
        + usize::from(custom_warn)
        + usize::from(tracked_term_warn)
        + usize::from(learned_filter_warn)
        + usize::from(!repeated_starts.is_empty())
        + usize::from(!subject_leads_rows.is_empty())
        + usize::from(!paragraph_leads_rows.is_empty())
        + usize::from(!sentence_patterns.is_empty())
        + usize::from(!judgement_endings.is_empty())
        + usize::from(!clause_prefixes.is_empty())
        + usize::from(!parallel_clauses.is_empty())
        + usize::from(aa_bb_warn)
        + usize::from(ba_operation_context_warn)
        + usize::from(sentence_lengths.warn)
        + usize::from(modifier_pressure.iter().any(|m| m.warn))
        + usize::from(!short_phrases.is_empty())
        + usize::from(dialogue_warn)
        + usize::from(scene_map.warn)
        + usize::from(dialogue_emotions.flatness_warn || dialogue_emotions.volatility_warn)
        + usize::from(battle_profile.warn)
        + usize::from(viewpoint_profile.warn)
        + usize::from(!dialogue.short_quote_runs.is_empty())
        + usize::from(!dialogue.question_ping_pong.is_empty())
        + usize::from(!dialogue.quote_ping_pong.is_empty())
        + usize::from(ending_warn)
        + usize::from(!fatigue_windows.is_empty())
        + usize::from(judgement_context_warn)
        + usize::from(!tracked_term_windows.is_empty());
    let summary = Summary {
        chars,
        sentences: sentence_infos.len(),
        paragraphs: paragraphs.len(),
        avg_sentence_chars: round2(chars as f64 / sentence_infos.len().max(1) as f64),
        short_sentences: sentence_lengths.short_count,
        very_short_sentences: sentence_lengths.very_short_count,
        short_sentence_ratio: sentence_lengths.short_ratio,
        quote_ratio: round4f(quote_ratio(text)),
        warn_sections,
    };

    let mut analysis = Analysis {
        source: source.to_string(),
        summary,
        warned,
        tokens: token_metrics,
        tracked_terms: tracked_term_metrics,
        tracked_term_categories,
        tracked_term_windows,
        tracked_term_window_count,
        ba_operation_contexts,
        patterns: pattern_metrics,
        phrases: phrase_metrics,
        modifiers: modifier_metrics,
        punctuation: punctuation_metrics,
        punctuation_combos: combo_metrics,
        custom_templates: custom_template_metrics,
        learned_filters: learned_filter_metrics,
        corpus_profile: corpus_profile_json(corpus_profile),
        dominant_punctuation,
        sentence_starts: repeated_starts,
        subject_leads: subject_leads_rows,
        paragraph_leads: paragraph_leads_rows,
        sentence_patterns,
        judgement_endings,
        clause_prefixes,
        parallel_clauses,
        aa_bb_patterns,
        sentence_lengths,
        fatigue_windows,
        fatigue_window_count,
        judgement_contexts,
        modifier_pressure,
        terms,
        short_phrases,
        dialogue,
        scene_map,
        dialogue_emotions,
        character_voice,
        tone_profile,
        battle_profile,
        viewpoint_profile,
        ending,
        template_candidates,
        hard_flags,
        style_fatigue: Vec::new(),
        review_reminders: Vec::new(),
    };
    analysis.style_fatigue = build_style_fatigue(&analysis);
    analysis.review_reminders = build_review_reminders(&analysis);
    Ok(analysis)
}
