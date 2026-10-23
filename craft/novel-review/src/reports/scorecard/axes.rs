//! 评审轴、加分候选与门禁判定（`build_axes` 8 条固定轴、`decide_gate`）。

use super::*;

/// 章末标签展示名（对齐 `stats.draft.ending_label_display` 的查表语义）。
pub(crate) fn ending_display(labels: &EndingLabels, label: &str) -> String {
    labels
        .display
        .get(label)
        .cloned()
        .unwrap_or_else(|| label.to_string())
}

fn clamp_score(value: i32) -> i32 {
    value.clamp(1, 5)
}

fn axis_score(base: i32, penalties: &[i32], bonuses: &[i32]) -> i32 {
    let total = base - penalties.iter().sum::<i32>() + bonuses.iter().sum::<i32>();
    clamp_score(total)
}

/// 加分候选（`{name, reason}`）。
#[derive(Debug, Clone)]
pub struct BonusCandidate {
    pub name: String,
    pub reason: String,
}

/// `build_bonus_candidates`（风格侧，截前 4）。
pub fn build_bonus_candidates(analysis: &Analysis) -> Vec<BonusCandidate> {
    let mut candidates: Vec<BonusCandidate> = Vec::new();
    let summary = &analysis.summary;
    let dialogue = &analysis.dialogue;

    if summary.warn_sections <= 6 && analysis.style_fatigue.len() <= 3 {
        candidates.push(BonusCandidate {
            name: "整体收敛较稳".to_string(),
            reason: "硬警告分区不多，句式疲劳家族数量也较少，说明这一章的文体控制相对稳定。"
                .to_string(),
        });
    }
    if (0.12..=0.45).contains(&summary.quote_ratio) && dialogue.dialogue_axis_gaps.is_empty() {
        candidates.push(BonusCandidate {
            name: "对白与叙述配比自然".to_string(),
            reason: "对白比例在可读区间内，且没有明显对白转轴缺口，说明场面没有塌成纯互答录音。"
                .to_string(),
        });
    }
    if (0.18..=0.35).contains(&summary.short_sentence_ratio) && !analysis.sentence_lengths.warn {
        candidates.push(BonusCandidate {
            name: "短句节奏可保留".to_string(),
            reason: "短句比例存在但没有连发失控，更像节奏设计而不是内容写薄。".to_string(),
        });
    }
    if !analysis.ending.warn && !analysis.ending.tail_excerpt.is_empty() {
        candidates.push(BonusCandidate {
            name: "章末收束未模板化".to_string(),
            reason: "章末没有落入现有意象/流程词模板，说明结尾功能有继续发展的空间。".to_string(),
        });
    }
    if !analysis.aa_bb_patterns.is_empty() && !analysis.aa_bb_patterns.iter().any(|p| p.warn) {
        candidates.push(BonusCandidate {
            name: "局部排比可视作风格点缀".to_string(),
            reason: "检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。"
                .to_string(),
        });
    }
    let scene_map = &analysis.scene_map;
    if !scene_map.warn && scene_map.switch_count >= 2 {
        candidates.push(BonusCandidate {
            name: "场面功能有切换".to_string(),
            reason: "粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。".to_string(),
        });
    }
    let dialogue_emotions = &analysis.dialogue_emotions;
    if dialogue_emotions.dialogue_sentences >= 4
        && !dialogue_emotions.flatness_warn
        && !dialogue_emotions.volatility_warn
        && dialogue_emotions.shift_count >= 1
    {
        candidates.push(BonusCandidate {
            name: "对白情绪有起伏".to_string(),
            reason: "对白情绪不是单一平推，也没有明显横跳，更像在做关系推进而不是纯互顶。"
                .to_string(),
        });
    }
    let battle_profile = &analysis.battle_profile;
    if battle_profile.sequence_count >= 1 && battle_profile.result_ratio >= 0.35 {
        candidates.push(BonusCandidate {
            name: "动作段有后果反馈".to_string(),
            reason: "冲突段不只累计动作动词，也带出了结果、伤害或位移反馈，可以视作紧凑度候选。"
                .to_string(),
        });
    }
    candidates.truncate(4);
    candidates
}

/// `build_consistency_bonus_candidates`（一致性侧，截前 2）。
pub(crate) fn build_consistency_bonus_candidates(
    snapshot: Option<&StoryConflictSnapshot>,
) -> Vec<BonusCandidate> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if !snapshot.available {
        return Vec::new();
    }
    let mut candidates: Vec<BonusCandidate> = Vec::new();
    if snapshot.decision_counter.get("designed_keep") >= 1 {
        candidates.push(BonusCandidate {
            name: "一致性例外已沉淀".to_string(),
            reason: "同一 story 已有候选被人工判为设计性保留，说明工具开始学会区分“可保留的变化”和“真漂移”。".to_string(),
        });
    }
    if snapshot.decision_counter.get("false_positive") >= 1 && snapshot.pending_rows.is_empty() {
        candidates.push(BonusCandidate {
            name: "一致性复核收敛".to_string(),
            reason: "这一条 story 的一致性候选已有复核反馈，且当前没有遗留待判项，说明复审闭环在起作用。".to_string(),
        });
    }
    candidates.truncate(2);
    candidates
}

/// 评审轴（`{name, score, reason}`）。
#[derive(Debug, Clone)]
pub struct Axis {
    pub name: String,
    pub score: i32,
    pub reason: String,
}

/// `build_axes`：8 条固定轴（名称/说明固定）。
pub fn build_axes(
    analysis: &Analysis,
    consistency_snapshot: Option<&StoryConflictSnapshot>,
    alignment_snapshot: Option<&alignment::Alignment>,
    story_trend_snapshot: Option<&TrendSnapshot>,
) -> Vec<Axis> {
    let summary = &analysis.summary;
    let dialogue = &analysis.dialogue;
    let fatigue_warn_count = analysis
        .style_fatigue
        .iter()
        .filter(|item| item.status == "WARN")
        .count();
    let reminder_p1_count = analysis
        .review_reminders
        .iter()
        .filter(|item| item.priority == "P1")
        .count();
    let reminder_p2_count = analysis
        .review_reminders
        .iter()
        .filter(|item| item.priority == "P2")
        .count();
    let dialogue_gap_count = dialogue.dialogue_axis_gaps.len();
    let ping_pong_count = dialogue.quote_ping_pong.len() + dialogue.question_ping_pong.len();
    let scene_map = &analysis.scene_map;
    let dialogue_emotions = &analysis.dialogue_emotions;
    let character_voice = &analysis.character_voice;
    let tone_profile = &analysis.tone_profile;
    let battle_profile = &analysis.battle_profile;
    let viewpoint_profile = &analysis.viewpoint_profile;
    let mut consistency_pending = 0usize;
    let mut consistency_confirmed = 0usize;
    let mut consistency_false_positive = 0usize;
    let mut alignment_mismatch_count = 0usize;
    if let Some(consistency_snapshot) = consistency_snapshot {
        if consistency_snapshot.available {
            consistency_pending = consistency_snapshot.pending_rows.len();
            consistency_confirmed = consistency_snapshot.decision_counter.get("confirmed");
            consistency_false_positive =
                consistency_snapshot.decision_counter.get("false_positive");
        }
    }
    if let Some(alignment_snapshot) = alignment_snapshot {
        if alignment_snapshot.available {
            alignment_mismatch_count = alignment_snapshot.mismatch_count.unwrap_or(0);
        }
    }
    let convergence_kinds: Vec<String> = story_trend_snapshot
        .map(|t| t.convergence_kinds.clone())
        .unwrap_or_default();

    let mut axes: Vec<Axis> = Vec::new();

    let repetition_score = axis_score(
        5,
        &[
            ((analysis.hard_flags.len() / 6).min(3)) as i32,
            fatigue_warn_count.min(2) as i32,
            i32::from(analysis.tracked_term_window_count >= 3),
            i32::from(analysis.sentence_patterns.len() >= 4),
        ],
        &[i32::from(summary.warn_sections <= 4)],
    );
    axes.push(Axis {
        name: "重复控制".to_string(),
        score: repetition_score,
        reason: "综合硬警告、句式疲劳、局部点名密度与句首骨架重复。".to_string(),
    });

    let sentence_score = axis_score(
        5,
        &[
            i32::from(analysis.sentence_lengths.warn),
            i32::from(analysis.clause_prefixes.len() >= 4),
            i32::from(analysis.parallel_clauses.len() >= 3),
            i32::from(analysis.aa_bb_patterns.iter().any(|p| p.warn)),
        ],
        &[i32::from(
            (0.18..=0.35).contains(&summary.short_sentence_ratio),
        )],
    );
    axes.push(Axis {
        name: "句式弹性".to_string(),
        score: sentence_score,
        reason: "观察短句、并列分句、AA/BB 节奏和分句前缀，判断是节奏还是手癖。".to_string(),
    });

    let dialogue_score = axis_score(
        5,
        &[
            dialogue_gap_count.min(2) as i32,
            i32::from(ping_pong_count >= 2),
            i32::from(dialogue.dense_quote_run_count >= 2),
            i32::from(summary.quote_ratio > 0.55),
            i32::from(dialogue_emotions.flatness_warn),
            i32::from(dialogue_emotions.volatility_warn),
            i32::from(character_voice.warn),
            i32::from(convergence_kinds.iter().any(|k| k == "ending_emotion")),
        ],
        &[
            i32::from((0.12..=0.45).contains(&summary.quote_ratio) && dialogue_gap_count == 0),
            i32::from(dialogue_emotions.shift_count >= 1 && !dialogue_emotions.volatility_warn),
            i32::from(!character_voice.warn && character_voice.speaker_count >= 2),
        ],
    );
    axes.push(Axis {
        name: "对白情感与转轴".to_string(),
        score: dialogue_score,
        reason: "看对白是否有动作、环境、第三方或设备转轴，而不是长时间互顶。".to_string(),
    });

    let tone_score = axis_score(
        4,
        &[
            i32::from(analysis.ending.warn),
            i32::from(analysis.modifier_pressure.iter().any(|m| m.warn)),
            i32::from(analysis.fatigue_windows.len() >= 4),
            i32::from(tone_profile.warn),
            i32::from(scene_map.warn),
            i32::from(convergence_kinds.iter().any(|k| k == "ending_tone")),
        ],
        &[
            i32::from(!analysis.ending.warn),
            i32::from(tone_profile.stable_ratio >= 0.35 && tone_profile.dominant_tone != "none"),
        ],
    );
    axes.push(Axis {
        name: "场景色调稳定".to_string(),
        score: tone_score,
        reason: "暂时用章末模板、修饰压力和局部疲劳窗口做代理指标，后续再接更细的色调分类。"
            .to_string(),
    });

    let tension_score = axis_score(
        4,
        &[
            i32::from(summary.short_sentence_ratio > 0.42),
            i32::from(ping_pong_count >= 2),
            i32::from(dialogue_gap_count >= 2),
            i32::from(battle_profile.warn),
        ],
        &[
            i32::from(summary.avg_sentence_chars >= 14.0 && summary.avg_sentence_chars <= 28.0),
            i32::from(battle_profile.sequence_count >= 1 && battle_profile.result_ratio >= 0.35),
        ],
    );
    axes.push(Axis {
        name: "张力与紧凑度".to_string(),
        score: tension_score,
        reason: "用句长、对白互顶和转轴缺口粗看战斗/冲突段是否只是快而不紧。".to_string(),
    });

    let viewpoint_score = axis_score(
        4,
        &[
            i32::from(analysis.judgement_contexts.len() >= 3),
            i32::from(analysis.learned_filters.len() >= 4),
            i32::from(reminder_p1_count >= 3),
            i32::from(viewpoint_profile.warn),
        ],
        &[
            i32::from(reminder_p1_count == 0),
            i32::from(!viewpoint_profile.warn && !viewpoint_profile.dominant_anchor.is_empty()),
        ],
    );
    axes.push(Axis {
        name: "视角与判断稳定".to_string(),
        score: viewpoint_score,
        reason: "当前主要用判断句上下文、语料偏移和高优先提醒做代理，先拦旁白抢跑与说明过重。"
            .to_string(),
    });

    let consistency_score = axis_score(
        4,
        &[
            i32::from(analysis.tracked_term_window_count >= 4),
            i32::from(analysis.learned_filters.len() >= 5),
            i32::from(summary.warn_sections >= 12),
            i32::from(consistency_pending >= 1),
            i32::from(consistency_confirmed >= 2),
            i32::from(alignment_mismatch_count >= 2),
        ],
        &[
            i32::from(analysis.corpus_profile.enabled),
            i32::from(consistency_false_positive >= 1 && consistency_pending == 0),
            i32::from(
                alignment_mismatch_count == 0
                    && matches!(
                        alignment_snapshot,
                        Some(alignment) if alignment.available
                    ),
            ),
        ],
    );
    axes.push(Axis {
        name: "一致性准备度".to_string(),
        score: consistency_score,
        reason: "看当前章与同书语料的偏离程度，以及当前 story 的一致性候选是否已被复核、确认或仍待处理。".to_string(),
    });

    let structure_score = axis_score(
        4,
        &[
            i32::from(reminder_p1_count >= 2),
            i32::from(analysis.fatigue_windows.len() >= 5),
            i32::from(summary.warn_sections >= 14),
            i32::from(scene_map.warn),
            i32::from(alignment_mismatch_count >= 1),
        ],
        &[
            i32::from(reminder_p1_count == 0 && reminder_p2_count <= 2),
            i32::from(scene_map.switch_count >= 2 && !scene_map.warn),
            i32::from(
                alignment_mismatch_count == 0
                    && matches!(
                        alignment_snapshot,
                        Some(alignment) if alignment.available
                    ),
            ),
        ],
    );
    axes.push(Axis {
        name: "结构完成度".to_string(),
        score: structure_score,
        reason: "暂用高优先提醒、局部高压窗口和总体告警量做代理，后续再接 Scene/章末功能分析。"
            .to_string(),
    });
    axes
}

/// `decide_gate`：门禁（gate / priority / recommendation）。
pub fn decide_gate(analysis: &Analysis, axes: &[Axis]) -> (String, String, String) {
    let p1_count = analysis
        .review_reminders
        .iter()
        .filter(|item| item.priority == "P1")
        .count();
    let avg_score =
        axes.iter().map(|axis| axis.score as f64).sum::<f64>() / axes.len().max(1) as f64;
    let hard_count = analysis.hard_flags.len();
    let warn_sections = analysis.summary.warn_sections;

    if p1_count >= 4 || warn_sections >= 16 || avg_score < 2.4 || hard_count >= 22 {
        return (
            "FAIL".to_string(),
            "P0".to_string(),
            "targeted_rewrite".to_string(),
        );
    }
    if p1_count >= 2 || warn_sections >= 10 || avg_score < 3.4 || hard_count >= 12 {
        return (
            "WATCH".to_string(),
            "P1".to_string(),
            "light_revise".to_string(),
        );
    }
    ("PASS".to_string(), "P2".to_string(), "retain".to_string())
}
