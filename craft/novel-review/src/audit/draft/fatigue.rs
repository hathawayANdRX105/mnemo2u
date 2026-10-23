//! 局部疲劳窗口与风格疲劳（对齐 build_fatigue_windows / build_style_fatigue）。

use super::*;

/// 局部疲劳窗口（对齐 `build_fatigue_windows`：滑窗 5、块 ≥4 句才参与评分）。
pub(crate) fn build_fatigue_windows(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> Vec<FatigueWindow> {
    if sentence_infos.len() < 4 {
        return Vec::new();
    }
    let window_size = 5;
    let short_max = ctx.thresholds().short_sentence_max_chars as usize;
    let very_short_max = ctx.thresholds().very_short_sentence_max_chars as usize;
    let lex = ctx.lexicon();
    let judgement_terms = &lex.fatigue_window_judgement_terms;
    let sticky_terms = &lex.fatigue_window_sticky_terms;

    let mut candidates: Vec<FatigueWindow> = Vec::new();
    for start in 0..sentence_infos
        .len()
        .saturating_sub(window_size)
        .saturating_add(1)
    {
        let chunk =
            &sentence_infos[start..start.saturating_add(window_size).min(sentence_infos.len())];
        if chunk.len() < 4 {
            continue;
        }
        let short_count = chunk.iter().filter(|i| i.chars <= short_max).count();
        let very_short_count = chunk.iter().filter(|i| i.chars <= very_short_max).count();
        let mut judgement_count = 0usize;
        let mut ba_count = 0usize;
        let mut sticky_count = 0usize;
        let mut role_lead_count = 0usize;
        let mut dialogue_count = 0usize;
        let mut listish_count = 0usize;
        for item in chunk {
            let stripped = lstrip_chars(&item.text, LEADING_PUNCT);
            if judgement_terms
                .iter()
                .any(|t| stripped.contains(t.as_str()))
            {
                judgement_count += 1;
            }
            if ctx.ba_regex.is_match(stripped).unwrap_or(false) {
                ba_count += 1;
            }
            if sticky_terms.iter().any(|t| stripped.contains(t.as_str())) {
                sticky_count += 1;
            }
            if lex
                .subject_leads
                .iter()
                .any(|p| stripped.starts_with(p.as_str()))
            {
                role_lead_count += 1;
            }
            if stripped.starts_with('\u{201c}')
                || stripped.starts_with("【Pi】")
                || stripped.contains("\u{ff1a}\u{201c}")
            {
                dialogue_count += 1;
            }
            if stripped.matches('，').count() + stripped.matches('、').count() >= 3 {
                listish_count += 1;
            }
        }
        let mut reasons: Vec<String> = Vec::new();
        if short_count >= 3 {
            reasons.push(format!("短句 {short_count}/5"));
        }
        if very_short_count >= 2 {
            reasons.push(format!("极短句 {very_short_count}/5"));
        }
        if judgement_count >= 2 {
            reasons.push(format!("判断解释 {judgement_count}/5"));
        }
        if ba_count >= 2 {
            reasons.push(format!("把字操作 {ba_count}/5"));
        }
        if sticky_count >= 2 {
            reasons.push(format!("黏糊词 {sticky_count}/5"));
        }
        if role_lead_count >= 3 {
            reasons.push(format!("角色起手 {role_lead_count}/5"));
        }
        if dialogue_count >= 4 {
            reasons.push(format!("对白挤压 {dialogue_count}/5"));
        }
        if listish_count >= 2 {
            reasons.push(format!("清单分句 {listish_count}/5"));
        }
        if reasons.is_empty() {
            continue;
        }
        let score = short_count * 2
            + very_short_count
            + judgement_count * 2
            + ba_count * 2
            + sticky_count
            + role_lead_count
            + dialogue_count
            + listish_count;
        let roles = summarize_short_roles(ctx, &chunk.iter().collect::<Vec<_>>());
        candidates.push(FatigueWindow {
            start_index: chunk[0].index,
            end_index: chunk[chunk.len() - 1].index,
            start_line: chunk[0].line_no,
            end_line: chunk[chunk.len() - 1].line_no,
            score,
            reasons,
            roles: roles.clone(),
            suggestion: suggest_short_run_action(&roles),
            sample: chunk.iter().map(|i| i.text.clone()).collect(),
            total_candidates: 0,
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.start_index.cmp(&b.start_index))
    });
    let total_candidates = candidates.len();
    let mut kept: Vec<FatigueWindow> = Vec::new();
    let mut occupied: HashSet<usize> = HashSet::new();
    for c in candidates {
        if (c.start_index..=c.end_index).any(|i| occupied.contains(&i)) {
            continue;
        }
        for i in c.start_index..=c.end_index {
            occupied.insert(i);
        }
        let mut w = c;
        w.total_candidates = total_candidates;
        kept.push(w);
        if kept.len() >= sample_limit {
            break;
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// 风格疲劳与审查提醒（对齐 build_style_fatigue / build_review_reminders）

/// 指标证据行（对齐 `_metric_evidence`：样本优先，否则 `count=/per_10k=`）。
pub(crate) fn metric_evidence(count: usize, per_10k: f64, samples: &[Hit]) -> String {
    if let Some(sample) = samples.first() {
        format!("L{} {}", sample.line_no, sample.snippet)
    } else {
        format!("count={count}, per_10k={}", float_repr(per_10k))
    }
}

/// 按展示名过滤规则指标（对齐 `_metrics_named`：name ∈ 集合且 count > 0）。
pub(crate) fn regex_metrics_named(list: &[RegexMetric], names: &[&str]) -> Vec<RegexMetric> {
    list.iter()
        .filter(|m| names.contains(&m.name.as_str()) && m.count > 0)
        .cloned()
        .collect()
}

/// 按展示名过滤跟踪词指标（对齐 `_metrics_named` 在 tracked_terms 节）。
fn tracked_metrics_named(
    list: &[crate::rules::TrackedMetric],
    names: &[&str],
) -> Vec<crate::rules::TrackedMetric> {
    list.iter()
        .filter(|m| names.contains(&m.name.as_str()) && m.count > 0)
        .cloned()
        .collect()
}

/// 疲劳状态（对齐 `_fatigue_status`）。
fn fatigue_status(warn: bool, count: usize, watch_at: usize) -> &'static str {
    if warn {
        "WARN"
    } else if count >= watch_at {
        "WATCH"
    } else {
        "OK"
    }
}

/// 指标计数合计（对齐 `_metric_count`）。
fn metric_count(list: &[RegexMetric]) -> usize {
    list.iter().map(|m| m.count).sum()
}

/// 去重且保序的证据（对齐 `_unique_evidence`：空串丢弃，限长）。
pub(crate) fn unique_evidence(items: &[String], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if item.is_empty() || !seen.insert(item) {
            continue;
        }
        out.push(item.clone());
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// 风格疲劳行（对齐 `build_style_fatigue`；基于已装配的 analysis 各节汇总）。
pub fn build_style_fatigue(a: &Analysis) -> Vec<FatigueRow> {
    let mut rows: Vec<FatigueRow> = Vec::new();
    let mut add = |family: &str,
                   status: &str,
                   count: usize,
                   risk: &str,
                   reduce: &str,
                   evidence: Vec<String>| {
        rows.push(FatigueRow {
            family: family.to_string(),
            status: status.to_string(),
            count,
            risk: risk.to_string(),
            reduce: reduce.to_string(),
            evidence: unique_evidence(&evidence, 3),
        });
    };

    let pi_metrics = regex_metrics_named(&a.patterns, &["Pi竖线状态栏", "Pi是否菜单"]);
    add(
        "Pi UI/菜单句",
        fatigue_status(
            pi_metrics.iter().any(|m| m.warn),
            metric_count(&pi_metrics),
            1,
        ),
        metric_count(&pi_metrics),
        "Pi 像系统面板，会削弱搭档感和人物反应。",
        "只留最有角色感的一处，其余改成卡顿、延迟、误读或人物自行判断。",
        pi_metrics
            .iter()
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect(),
    );

    let clue_metrics = regex_metrics_named(&a.patterns, &["线索面板词"]);
    add(
        "线索面板句",
        fatigue_status(
            clue_metrics.iter().any(|m| m.warn),
            metric_count(&clue_metrics),
            3,
        ),
        metric_count(&clue_metrics),
        "线索被归档、首屏、标签、坐标、重合等词收拢，读感像任务列表。",
        "把完整结论拆成发现、排除、误判、半确认，章末用行动阻力收束。",
        clue_metrics
            .iter()
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect(),
    );

    let conclusion_metrics = regex_metrics_named(&a.patterns, &["这不是X是Y"]);
    let negation_metrics = regex_metrics_named(
        &a.patterns,
        &[
            "不是A而是B",
            "不是A只是B/更像B",
            "肯定后否定",
            "否定后肯定",
            "问题在于/这就是",
        ],
    );
    let negation_tokens = regex_metrics_named(&a.tokens, &["不是", "只是", "而是"]);
    let negation_total = metric_count(
        &conclusion_metrics
            .iter()
            .chain(negation_metrics.iter())
            .chain(negation_tokens.iter())
            .cloned()
            .collect::<Vec<_>>(),
    );
    add(
        "否定/肯定判断句",
        fatigue_status(
            !conclusion_metrics.is_empty() || negation_metrics.iter().any(|m| m.warn),
            negation_total,
            4,
        ),
        negation_total,
        "不是/只是/而是/这就是一类句子会让旁白替读者解释。",
        "人物台词可保留；旁白判断改成动作、证据、误读或后果。",
        conclusion_metrics
            .iter()
            .chain(negation_metrics.iter())
            .chain(negation_tokens.iter())
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect(),
    );

    let narration_context = a
        .judgement_contexts
        .iter()
        .find(|item| item.context == "narration");
    if let Some(nc) = narration_context {
        add(
            "旁白判断句",
            fatigue_status(nc.warn, nc.count, 2),
            nc.count,
            "判断词集中在旁白里时，作者会替读者完成理解。",
            "人物台词可保留；旁白判断优先换成动作、证据、误读或后果。",
            nc.samples
                .iter()
                .take(3)
                .map(|s| format!("L{} {}：{}", s.line_no, s.terms.join(","), s.text))
                .collect(),
        );
    }

    let assertive_metrics = regex_metrics_named(&a.patterns, &["肯定判断/解释腔"]);
    let cliche_metrics = regex_metrics_named(
        &a.phrases,
        &["不是因为", "问题不在", "看起来", "更像", "像是", "至少"],
    );
    let cliche_total = metric_count(
        &assertive_metrics
            .iter()
            .chain(cliche_metrics.iter())
            .cloned()
            .collect::<Vec<_>>(),
    );
    add(
        "陈词/解释腔",
        fatigue_status(
            assertive_metrics.iter().any(|m| m.warn) || cliche_metrics.iter().any(|m| m.warn),
            cliche_total,
            3,
        ),
        cliche_total,
        "真正、其实、显然、更像、至少等解释词过密时，旁白会变成评语。",
        "删掉只负责解释的句子，改成角色误读、物件变化或场面后果。",
        assertive_metrics
            .iter()
            .chain(cliche_metrics.iter())
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect(),
    );

    let sl = &a.sentence_lengths;
    add(
        "短句/极短句",
        if sl.warn { "WARN" } else { "OK" },
        sl.short_count,
        "短句连发会把动作、情绪和信息压成碎拍。",
        "每个短句连发块只保留一个节拍点，其余改成动作因果或场面阻力。",
        {
            let base = format!(
                "short={}, very_short={}, ratio={}, runs={}",
                sl.short_count,
                sl.very_short_count,
                float_repr(sl.short_ratio),
                sl.short_runs.len()
            );
            if let Some(first) = sl.short_runs.first() {
                vec![
                    base,
                    format!(
                        "短句连发类型：{}；建议：{}",
                        format_short_roles(&first.roles, "，"),
                        first.suggestion
                    ),
                ]
            } else {
                vec![base]
            }
        },
    );

    add(
        "局部疲劳窗口",
        fatigue_status(!a.fatigue_windows.is_empty(), a.fatigue_windows.len(), 1),
        a.fatigue_windows.len(),
        "短句、解释、把字操作和角色起手在同一小段叠加时，读感会突然变累。",
        "先改分数最高的窗口：保留一个节奏点，其余改成动作因果、环境反应或视角切换。",
        a.fatigue_windows
            .iter()
            .take(3)
            .map(|item| {
                format!(
                    "S{}-{} L{}-{} {}；类型：{}；建议：{}",
                    item.start_index,
                    item.end_index,
                    item.start_line,
                    item.end_line,
                    item.reasons.join("、"),
                    format_short_roles(&item.roles, "，"),
                    item.suggestion
                )
            })
            .collect(),
    );

    let ba_metrics = regex_metrics_named(&a.patterns, &["把字操作句"]);
    let ba_contexts = &a.ba_operation_contexts;
    let mut ba_evidence: Vec<String> = ba_metrics
        .iter()
        .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
        .collect();
    for item in ba_contexts.iter().take(3) {
        let mut sample = String::new();
        if let Some(first) = item.samples.first() {
            sample = format!("L{} {}", first.line_no, first.snippet);
        }
        let mut line = format!("{} x{}；建议：{}", item.role, item.count, item.suggestion);
        if !sample.is_empty() {
            line.push('；');
            line.push_str(&sample);
        }
        ba_evidence.push(line);
    }
    let ba_total = std::cmp::max(
        metric_count(&ba_metrics),
        ba_contexts.iter().map(|c| c.count).sum(),
    );
    add(
        "把字操作句",
        fatigue_status(
            ba_metrics.iter().any(|m| m.warn) || ba_contexts.iter().any(|c| c.warn),
            ba_total,
            10,
        ),
        ba_total,
        "把 X 拖上/放进/压住/推过去过密，会像操作日志。",
        "工具操作保留；情绪和线索操作改成视觉结果、环境反应或被动阻力。",
        ba_evidence,
    );

    let simile_metrics = regex_metrics_named(&a.patterns, &["像/活像模板"]);
    add(
        "像/活像比喻",
        fatigue_status(
            simile_metrics.iter().any(|m| m.warn),
            metric_count(&simile_metrics),
            3,
        ),
        metric_count(&simile_metrics),
        "比喻模板过密会替代真实动作，让旁白解释气氛。",
        "每章只保留少数最有新意的比喻，其余改成具体动作、声音或物件变化。",
        simile_metrics
            .iter()
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect(),
    );

    let sticky_metrics = regex_metrics_named(
        &a.modifiers,
        &[
            "微微",
            "轻轻",
            "慢慢",
            "有点",
            "一点点",
            "显得",
            "过于",
            "几乎",
            "几乎没有",
        ],
    );
    add(
        "黏糊词/弱判断",
        fatigue_status(
            sticky_metrics.iter().any(|m| m.warn),
            metric_count(&sticky_metrics),
            4,
        ),
        metric_count(&sticky_metrics),
        "轻轻、微微、有点、显得等词过密时，动作力度会被磨软。",
        "优先删弱判断词；用动作幅度、声音、阻力来表示轻重。",
        sticky_metrics
            .iter()
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect(),
    );

    let aa_bb_warns: Vec<&AaBbPattern> = a.aa_bb_patterns.iter().filter(|i| i.warn).collect();
    let aa_bb_count: usize = a.aa_bb_patterns.iter().map(|i| i.count).sum();
    add(
        "AA/BB 短排比",
        fatigue_status(!aa_bb_warns.is_empty(), aa_bb_count, 2),
        aa_bb_count,
        "短分句排比会把画面写成清单。",
        "保留一个节奏点，其余并入动作过程或拆给人物反应。",
        aa_bb_warns
            .iter()
            .filter_map(|item| item.samples.first().cloned())
            .collect(),
    );

    let lead_count = a
        .paragraph_leads
        .iter()
        .chain(a.subject_leads.iter())
        .map(|i| i.count)
        .max()
        .unwrap_or(0);
    let mut lead_evidence: Vec<String> = Vec::new();
    if !a.paragraph_leads.is_empty() {
        lead_evidence.push(format!(
            "段首 {}",
            a.paragraph_leads
                .iter()
                .take(3)
                .map(|i| format!("{} x{}", i.phrase, i.count))
                .collect::<Vec<_>>()
                .join("，")
        ));
    }
    if !a.subject_leads.is_empty() {
        lead_evidence.push(format!(
            "主语 {}",
            a.subject_leads
                .iter()
                .take(3)
                .map(|i| format!("{} x{}", i.phrase, i.count))
                .collect::<Vec<_>>()
                .join("，")
        ));
    }
    add(
        "角色名/他她起手",
        fatigue_status(lead_count >= 8, lead_count, 4),
        lead_count,
        "段落总从角色名或他她起步，会让镜头调度单一。",
        "每三到四个角色起手里，换一个空间、物件、声音或证据变化起手。",
        lead_evidence,
    );

    let tracked_warns = tracked_metrics_named(&a.tracked_terms, &[]);
    let _ = tracked_warns;
    let tracked_warn_list: Vec<&crate::rules::TrackedMetric> =
        a.tracked_terms.iter().filter(|m| m.warn).collect();
    let tracked_total: usize =
        tracked_warn_list.iter().map(|m| m.count).sum::<usize>() + a.tracked_term_windows.len();
    let mut tracked_evidence: Vec<String> = tracked_warn_list
        .iter()
        .take(4)
        .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
        .collect();
    for item in a.tracked_term_windows.iter().take(3) {
        tracked_evidence.push(format!(
            "S{}-{} L{}-{} {}；{}；建议：{}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.reasons.join("、"),
            format_tracked_term_counts(
                &item.terms.iter().take(3).cloned().collect::<Vec<_>>(),
                "，"
            ),
            item.suggestion
        ));
    }
    add(
        "高频词/点名册",
        fatigue_status(
            !tracked_warn_list.is_empty() || !a.tracked_term_windows.is_empty(),
            tracked_total,
            8,
        ),
        tracked_total,
        "人物名、地名、设备名过密时，叙述会像点名册或设定表。",
        "用动作、称谓、空间位置和具体物件轮换，不要只靠同一个名词推进。",
        tracked_evidence,
    );

    let d = &a.dialogue;
    let dialogue_count = d.short_quote_runs.len()
        + d.question_ping_pong.len()
        + d.quote_ping_pong.len()
        + d.dialogue_axis_gaps.len();
    let mut dialogue_evidence: Vec<String> = Vec::new();
    if let Some(first) = d.short_quote_runs.first() {
        dialogue_evidence.push(first.sample.join(" | "));
    }
    if let Some(first) = d.question_ping_pong.first() {
        dialogue_evidence.push(first.sample.join(" | "));
    }
    if let Some(first) = d.quote_ping_pong.first() {
        dialogue_evidence.push(first.sample.join(" | "));
    }
    for item in d.dialogue_axis_gaps.iter().take(2) {
        dialogue_evidence.push(format!(
            "S{}-{} L{}-{} {}；建议：{}",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.reasons.join("、"),
            item.suggestion
        ));
    }
    add(
        "对白乒乓",
        fatigue_status(dialogue_count >= 1, dialogue_count, 1),
        dialogue_count,
        "短对白连续互顶时，动作和场面会消失。",
        "每四句对白至少插入一个动作、环境变化或人物误读作为转轴。",
        dialogue_evidence,
    );

    let sm = &a.scene_map;
    let role_summary = if sm.role_counts.is_empty() {
        "无".to_string()
    } else {
        sm.role_counts
            .iter()
            .map(|(name, count)| format!("{name} x{count}"))
            .collect::<Vec<_>>()
            .join("，")
    };
    add(
        "场面功能分布",
        fatigue_status(sm.warn, sm.block_count, 4),
        sm.block_count,
        "如果整章长时间停在对白块或说明块，场面会失去功能切换。",
        "让信息、动作、环境和关系推进互相接力，不要让单一功能吃满整章。",
        vec![format!(
            "dominant={} ratio={}；{role_summary}",
            sm.dominant_role,
            float_repr(sm.dominance_ratio)
        )],
    );

    let de = &a.dialogue_emotions;
    let emotion_summary = if de.emotion_counts.is_empty() {
        "无".to_string()
    } else {
        de.emotion_counts
            .iter()
            .map(|(name, count)| format!("{name} x{count}"))
            .collect::<Vec<_>>()
            .join("，")
    };
    add(
        "对白情绪曲线",
        fatigue_status(de.flatness_warn || de.volatility_warn, de.shift_count, 2),
        de.dialogue_sentences,
        "对白如果长期只剩一种情绪，或情绪标签频繁横跳，关系推进会发假。",
        "检查台词是在逼问、回避、防御还是安抚，并补动作或停顿让情绪转折落地。",
        vec![format!(
            "dominant={} ratio={} shift={}；{emotion_summary}",
            de.dominant_emotion,
            float_repr(de.dominant_ratio),
            de.shift_count
        )],
    );

    let cv = &a.character_voice;
    let mut voice_evidence: Vec<String> = Vec::new();
    for item in cv.speakers.iter().take(3) {
        voice_evidence.push(format!(
            "{} line={} avg={} q={} short={} emotion={}",
            item.speaker,
            item.lines,
            float_repr(item.avg_chars),
            float_repr(item.question_ratio),
            float_repr(item.short_ratio),
            item.dominant_emotion
        ));
    }
    voice_evidence.extend(cv.homogenized_pairs.iter().take(2).cloned());
    add(
        "角色声音",
        fatigue_status(cv.warn, cv.speaker_count, 2),
        cv.identified_lines,
        "如果多名角色的对白节拍、问句率和情绪主导长期接近，人物会越来越像同一个人在说话。",
        "让不同角色在句长、追问强度、判断习惯和情绪入口上拉开距离。",
        voice_evidence,
    );

    let bp = &a.battle_profile;
    add(
        "动作结果链",
        fatigue_status(bp.warn, bp.sequence_count, 1),
        bp.action_hits,
        "冲突段如果只有动作没有结果、伤害或位移反馈，会像挥空的动作脚本。",
        "每段冲突至少补一个结果句：谁退了、谁失衡了、什么东西坏了、谁被迫改动作。",
        vec![format!(
            "sequences={} action={} result={} damage={} ratio={}",
            bp.sequence_count,
            bp.action_hits,
            bp.result_hits,
            bp.damage_hits,
            float_repr(bp.result_ratio)
        )],
    );

    let vp = &a.viewpoint_profile;
    let anchor_summary = if vp.anchor_counts.is_empty() {
        "无".to_string()
    } else {
        vp.anchor_counts
            .iter()
            .map(|(name, count)| format!("{name} x{count}"))
            .collect::<Vec<_>>()
            .join("，")
    };
    add(
        "视角锚点",
        fatigue_status(vp.warn, vp.overlap_count, 1),
        vp.switch_count,
        "同段多人物心理暴露或近距离切锚偏多时，镜头中心会发飘。",
        "近距离视角段先固定一个感知中心；别在同段同时替两个人解释内心。",
        vec![format!(
            "anchors={anchor_summary} switch={} overlap={}",
            vp.switch_count, vp.overlap_count
        )],
    );

    let en = &a.ending;
    let ending_count = en.image_terms.len() + en.flow_terms.len();
    let mut ending_evidence: Vec<String> = Vec::new();
    if !en.flow_terms.is_empty() {
        ending_evidence.push(format!(
            "流程词 {}",
            en.flow_terms
                .iter()
                .map(|t| format!("{} x{}", t.term, t.count))
                .collect::<Vec<_>>()
                .join("，")
        ));
    }
    if !en.image_terms.is_empty() {
        ending_evidence.push(format!(
            "意象词 {}",
            en.image_terms
                .iter()
                .map(|t| format!("{} x{}", t.term, t.count))
                .collect::<Vec<_>>()
                .join("，")
        ));
    }
    if !en.tail_excerpt.is_empty() {
        ending_evidence.push(prefix_chars(&en.tail_excerpt, 120));
    }
    add(
        "章末模板",
        fatigue_status(en.warn, ending_count, 2),
        ending_count,
        "章末反复用流程词或同类意象收束，会让钩子同质。",
        "在动作余波、关系变化、外部阻力三类里换一种收束手势。",
        ending_evidence,
    );

    let modifier_warns: Vec<&ModifierPressure> =
        a.modifier_pressure.iter().filter(|i| i.warn).collect();
    let modifier_total: usize = a.modifier_pressure.iter().map(|i| i.total).sum();
    add(
        "修饰/动作压力",
        fatigue_status(!modifier_warns.is_empty(), modifier_total, 80),
        modifier_total,
        "同类修饰词和动作词过密时，画面会发僵。",
        "优先改 dense_sentences，不要只替换同义词。",
        a.modifier_pressure
            .iter()
            .filter(|i| i.total > 0)
            .map(|i| format!("{} total={} dense={}", i.label, i.total, i.dense_sentences))
            .collect(),
    );

    let status_order = |status: &str| -> i32 {
        match status {
            "WARN" => 0,
            "WATCH" => 1,
            "OK" => 2,
            _ => 9,
        }
    };
    rows.sort_by(|a, b| {
        status_order(&a.status)
            .cmp(&status_order(&b.status))
            .then_with(|| a.family.cmp(&b.family))
    });
    rows
}
