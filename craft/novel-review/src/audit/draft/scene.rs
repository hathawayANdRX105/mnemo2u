//! 段落功能/场景地图、语气/战斗/视角画像（对齐 build_scene_map / build_battle_profile 等）。

use super::*;

/// 词表命中总数（对齐 `_count_term_hits`：`text.count(term)` 求和）。
fn count_term_hits(text: &str, terms: &[String]) -> usize {
    terms.iter().map(|t| text.matches(t.as_str()).count()).sum()
}

/// 段落功能分类（对齐 `classify_paragraph_role`）。
fn classify_paragraph_role(
    ctx: &DraftContext,
    paragraph: &crate::text::ParagraphInfo,
) -> &'static str {
    let stripped = lstrip_chars(paragraph.text.trim(), LEADING_PUNCT);
    if paragraph.is_dialogue {
        return "dialogue";
    }
    let lex = ctx.lexicon();
    let battle_terms: Vec<String> = lex
        .battle_action_terms
        .iter()
        .chain(lex.battle_damage_terms.iter())
        .chain(lex.battle_result_terms.iter())
        .cloned()
        .collect();
    let battle_hits = count_term_hits(stripped, &battle_terms);
    let action_hits = count_term_hits(stripped, &lex.verb_hints)
        + usize::from(ctx.ba_regex.is_match(stripped).unwrap_or(false));
    let info_hits =
        count_term_hits(stripped, &lex.paragraph_info_terms) + stripped.matches('\u{ff1a}').count();
    let emotion_terms: Vec<String> = lex
        .short_role_emotion_terms
        .iter()
        .chain(lex.mental_state_terms.iter())
        .cloned()
        .collect();
    let emotion_hits = count_term_hits(stripped, &emotion_terms);
    let tone_hits: usize = lex
        .tone_rules
        .rules
        .iter()
        .map(|(_, terms)| count_term_hits(stripped, terms))
        .sum();
    if battle_hits >= 2 {
        return "battle";
    }
    if info_hits >= 3 && info_hits >= action_hits {
        return "info";
    }
    if action_hits >= 3 && action_hits >= emotion_hits {
        return "action";
    }
    if emotion_hits >= 2 {
        return "emotion";
    }
    if tone_hits >= 2 {
        return "environment";
    }
    "mixed"
}

/// 收尾一个功能块（对齐 `build_scene_map` 内部 `flush`；拆成函数避免借用冲突）。
fn flush_scene_block(
    blocks: &mut Vec<SceneBlock>,
    current_role: &mut String,
    current_items: &mut Vec<&crate::text::ParagraphInfo>,
) {
    if current_items.is_empty() {
        return;
    }
    blocks.push(SceneBlock {
        role: if current_role.is_empty() {
            "mixed".to_string()
        } else {
            current_role.clone()
        },
        start_paragraph: current_items[0].index,
        end_paragraph: current_items.last().unwrap().index,
        start_line: current_items[0].line_start,
        end_line: current_items.last().unwrap().line_end,
        paragraphs: current_items.len(),
        chars: current_items.iter().map(|i| i.chars).sum(),
        sample: current_items
            .iter()
            .take(2)
            .map(|i| prefix_chars(i.text.replace('\n', " ").as_str(), 80))
            .collect(),
    });
    current_items.clear();
    current_role.clear();
}

/// 粗分块功能地图（对齐 `build_scene_map`：换功能 ≥2 块才切断；
/// 对白→非对白立即切；`scene_break_leads` 起手强切）。
pub fn build_scene_map(
    ctx: &DraftContext,
    paragraph_infos: &[crate::text::ParagraphInfo],
    sample_limit: usize,
) -> SceneMap {
    if paragraph_infos.is_empty() {
        return SceneMap {
            blocks: Vec::new(),
            role_counts: CountMap::default(),
            dominant_role: "mixed".into(),
            dominance_ratio: 0.0,
            warn: false,
            block_count: 0,
            switch_count: 0,
        };
    }
    let mut blocks: Vec<SceneBlock> = Vec::new();
    let mut current_role = String::new();
    let mut current_items: Vec<&crate::text::ParagraphInfo> = Vec::new();
    for info in paragraph_infos {
        let role = classify_paragraph_role(ctx, info);
        let stripped = lstrip_chars(info.text.trim(), LEADING_PUNCT);
        let lex = ctx.lexicon();
        let force_break = !current_items.is_empty()
            && (lex
                .scene_break_leads
                .iter()
                .any(|term| stripped.starts_with(term.as_str()))
                || (current_role == "dialogue" && role != "dialogue")
                || (current_role != role && current_items.len() >= 2));
        if force_break {
            flush_scene_block(&mut blocks, &mut current_role, &mut current_items);
        }
        if current_items.is_empty() {
            current_role = role.to_string();
        }
        current_items.push(info);
    }
    flush_scene_block(&mut blocks, &mut current_role, &mut current_items);
    let mut role_counter = Counter::default();
    for block in &blocks {
        role_counter.add(&block.role);
    }
    let (dominant_role, dominant_count) = role_counter
        .most_common(1)
        .into_iter()
        .next()
        .unwrap_or_else(|| ("mixed".to_string(), 0));
    let dominance_ratio = round4f(dominant_count as f64 / blocks.len().max(1) as f64);
    let warn = blocks.len() >= 4
        && (dominant_role == "dialogue" || dominant_role == "info")
        && dominance_ratio >= 0.6;
    let block_count = blocks.len();
    SceneMap {
        blocks: blocks.into_iter().take(sample_limit * 2).collect(),
        role_counts: CountMap::new(role_counter.entries().to_vec()),
        dominant_role,
        dominance_ratio,
        warn,
        block_count,
        switch_count: block_count.saturating_sub(1),
    }
}

/// 语气画像（对齐 `build_tone_profile`：活跃 label 取计数最高，平手取 label 序）。
pub fn build_tone_profile(
    ctx: &DraftContext,
    paragraph_infos: &[crate::text::ParagraphInfo],
    sample_limit: usize,
) -> ToneProfile {
    let mut tone_counter = Counter::default();
    let mut paragraph_tones: Vec<String> = Vec::new();
    let mut samples: Vec<ToneSample> = Vec::new();
    let mut switch_count = 0usize;
    let mut last_tone = String::new();
    for info in paragraph_infos {
        let stripped = info.text.trim();
        let lex = ctx.lexicon();
        let mut active: Vec<(&str, usize)> = Vec::new();
        for (label, terms) in &lex.tone_rules.rules {
            let count = count_term_hits(stripped, terms);
            if count > 0 {
                active.push((label.as_str(), count));
            }
        }
        if active.is_empty() {
            continue;
        }
        // 选最优条目：计数最高，平手按 label 升序取首。
        let best = active
            .iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
            .unwrap();
        tone_counter.add_n(best.0, best.1);
        paragraph_tones.push(best.0.to_string());
        if !last_tone.is_empty() && best.0 != last_tone {
            switch_count += 1;
        }
        last_tone = best.0.to_string();
        if samples.len() < sample_limit {
            samples.push(ToneSample {
                paragraph: info.index,
                line_no: info.line_start,
                tone: best.0.to_string(),
                text: prefix_chars(stripped, 80),
            });
        }
    }
    let (dominant_tone, dominant_count) = tone_counter
        .most_common(1)
        .into_iter()
        .next()
        .unwrap_or_else(|| ("none".to_string(), 0));
    let total = tone_counter.total();
    let stable_ratio = if total == 0 {
        0.0
    } else {
        round4f(dominant_count as f64 / total as f64)
    };
    let distinct_count: std::collections::HashSet<&String> =
        std::collections::HashSet::from_iter(paragraph_tones.iter());
    let warn = !paragraph_tones.is_empty()
        && ((distinct_count.len() >= 4 && switch_count >= (paragraph_tones.len() / 2).max(2))
            || dominant_tone == "none");
    ToneProfile {
        tone_counts: CountMap::new(tone_counter.entries().to_vec()),
        dominant_tone,
        stable_ratio,
        switch_count,
        samples,
        warn,
    }
}

/// 战斗段画像（对齐 `build_battle_profile`：连续战斗句成段，
/// warn = 动作 ≥3 且无结果且无伤害反馈）。
pub fn build_battle_profile(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> BattleProfile {
    let lex = ctx.lexicon();
    let all_battle: Vec<String> = lex
        .battle_action_terms
        .iter()
        .chain(lex.battle_damage_terms.iter())
        .chain(lex.battle_result_terms.iter())
        .cloned()
        .collect();
    let mut sequences: Vec<BattleSequence> = Vec::new();
    let mut current: Vec<&crate::text::SentenceInfo> = Vec::new();
    let mut previous_index = 0usize;
    for item in sentence_infos {
        if count_term_hits(&item.text, &all_battle) >= 1 {
            if !current.is_empty() && item.index != previous_index + 1 {
                if current.len() >= 2 {
                    let joined: String = current
                        .iter()
                        .map(|i| i.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    sequences.push(BattleSequence {
                        start_index: current[0].index,
                        end_index: current.last().unwrap().index,
                        start_line: current[0].line_no,
                        end_line: current.last().unwrap().line_no,
                        sentences: current.len(),
                        action_hits: count_term_hits(&joined, &lex.battle_action_terms),
                        result_hits: count_term_hits(&joined, &lex.battle_result_terms),
                        damage_hits: count_term_hits(&joined, &lex.battle_damage_terms),
                        movement_hits: count_term_hits(&joined, &lex.battle_movement_terms),
                        sample: current.iter().take(4).map(|i| i.text.clone()).collect(),
                        warn: false,
                    });
                    let last = sequences.last_mut().unwrap();
                    last.warn =
                        last.action_hits >= 3 && last.result_hits == 0 && last.damage_hits == 0;
                }
                current.clear();
            }
            current.push(item);
            previous_index = item.index;
        } else {
            if current.len() >= 2 {
                let joined: String = current
                    .iter()
                    .map(|i| i.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                sequences.push(BattleSequence {
                    start_index: current[0].index,
                    end_index: current.last().unwrap().index,
                    start_line: current[0].line_no,
                    end_line: current.last().unwrap().line_no,
                    sentences: current.len(),
                    action_hits: count_term_hits(&joined, &lex.battle_action_terms),
                    result_hits: count_term_hits(&joined, &lex.battle_result_terms),
                    damage_hits: count_term_hits(&joined, &lex.battle_damage_terms),
                    movement_hits: count_term_hits(&joined, &lex.battle_movement_terms),
                    sample: current.iter().take(4).map(|i| i.text.clone()).collect(),
                    warn: false,
                });
                let last = sequences.last_mut().unwrap();
                last.warn = last.action_hits >= 3 && last.result_hits == 0 && last.damage_hits == 0;
            }
            current.clear();
            previous_index = item.index;
        }
    }
    if current.len() >= 2 {
        let joined: String = current
            .iter()
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        sequences.push(BattleSequence {
            start_index: current[0].index,
            end_index: current.last().unwrap().index,
            start_line: current[0].line_no,
            end_line: current.last().unwrap().line_no,
            sentences: current.len(),
            action_hits: count_term_hits(&joined, &lex.battle_action_terms),
            result_hits: count_term_hits(&joined, &lex.battle_result_terms),
            damage_hits: count_term_hits(&joined, &lex.battle_damage_terms),
            movement_hits: count_term_hits(&joined, &lex.battle_movement_terms),
            sample: current.iter().take(4).map(|i| i.text.clone()).collect(),
            warn: false,
        });
        let last = sequences.last_mut().unwrap();
        last.warn = last.action_hits >= 3 && last.result_hits == 0 && last.damage_hits == 0;
    }
    let total_action: usize = sequences.iter().map(|s| s.action_hits).sum();
    let total_result: usize = sequences.iter().map(|s| s.result_hits).sum();
    let total_damage: usize = sequences.iter().map(|s| s.damage_hits).sum();
    let total_movement: usize = sequences.iter().map(|s| s.movement_hits).sum();
    let warn_sequences = sequences.iter().filter(|s| s.warn).count();
    let result_ratio = if total_action == 0 {
        0.0
    } else {
        round4f((total_result + total_damage) as f64 / total_action.max(1) as f64)
    };
    let warn = warn_sequences >= 2
        || (sequences.len() >= 3
            && warn_sequences as f64 / sequences.len().max(1) as f64 >= 0.5
            && total_action >= 6);
    BattleProfile {
        sequence_count: sequences.len(),
        max_sequence_sentences: sequences.iter().map(|s| s.sentences).max().unwrap_or(0),
        action_hits: total_action,
        result_hits: total_result,
        damage_hits: total_damage,
        movement_hits: total_movement,
        result_ratio,
        warn_sequences,
        warn,
        samples: sequences.into_iter().take(sample_limit).collect(),
    }
}

/// 视角锚点画像（对齐 `build_viewpoint_profile`：
/// 段内多锚 + 心理词 = 重叠；换锚计切）。
pub fn build_viewpoint_profile(
    ctx: &DraftContext,
    paragraph_infos: &[crate::text::ParagraphInfo],
    sample_limit: usize,
) -> ViewpointProfile {
    let lex = ctx.lexicon();
    let anchors = &lex.subject_leads;
    let mut overlaps: Vec<OverlapEntry> = Vec::new();
    let mut switches = 0usize;
    let mut last_anchor = String::new();
    let mut anchor_counter = Counter::default();
    let has_mental = lex.mental_state_terms.iter().any(|t| {
        let _ = t;
        false
    });
    let _ = has_mental;
    for info in paragraph_infos {
        let text = info.text.trim();
        let has_mental = lex
            .mental_state_terms
            .iter()
            .any(|t| text.contains(t.as_str()));
        let mut paragraph_anchors: Vec<String> = anchors
            .iter()
            .filter(|anchor| text.contains(anchor.as_str()) && has_mental)
            .cloned()
            .collect();
        paragraph_anchors.dedup();
        paragraph_anchors.sort();
        if paragraph_anchors.len() >= 2 && overlaps.len() < sample_limit {
            overlaps.push(OverlapEntry {
                paragraph: info.index,
                line_no: info.line_start,
                anchors: paragraph_anchors.clone(),
                text: prefix_chars(text, 100),
            });
        }
        if !paragraph_anchors.is_empty() {
            let primary = &paragraph_anchors[0];
            anchor_counter.add(primary);
            if !last_anchor.is_empty() && primary != &last_anchor {
                switches += 1;
            }
            last_anchor = primary.clone();
        }
    }
    let dominant_anchor = anchor_counter
        .most_common(1)
        .into_iter()
        .next()
        .map(|(k, _)| k)
        .unwrap_or_default();
    let warn = !overlaps.is_empty() || switches >= 3;
    ViewpointProfile {
        anchor_counts: CountMap::new(anchor_counter.entries().to_vec()),
        dominant_anchor,
        switch_count: switches,
        overlap_count: overlaps.len(),
        overlaps,
        warn,
    }
}
