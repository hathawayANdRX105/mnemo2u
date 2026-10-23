//! 审查提醒（对齐 build_review_reminders：把原始告警翻成复核问题）。

use super::*;

/// 审查提醒（对齐 `build_review_reminders`：把原始告警翻成复核问题）。
pub fn build_review_reminders(a: &Analysis) -> Vec<ReviewReminder> {
    let mut reminders: Vec<ReviewReminder> = Vec::new();
    let mut add = |priority: &str,
                   category: &str,
                   title: &str,
                   reason: &str,
                   check: &str,
                   action: &str,
                   evidence: Vec<String>| {
        reminders.push(ReviewReminder {
            priority: priority.to_string(),
            category: category.to_string(),
            title: title.to_string(),
            reason: reason.to_string(),
            check: check.to_string(),
            action: action.to_string(),
            evidence: unique_evidence(&evidence, 4),
        });
    };

    let pi_metrics = regex_metrics_named(&a.patterns, &["Pi竖线状态栏", "Pi是否菜单"]);
    if !pi_metrics.is_empty() {
        add(
            "P1",
            "Pi",
            "Pi 输出正在滑向 UI/菜单",
            "Pi 负责给结论或按钮提示时，会从搭档变成系统面板。",
            "检查 Pi 输出后是否还有人物误读、停顿、拒绝配合或行动后果。",
            "保留最有角色感的一处 Pi 文本，其余改成蓝字卡顿、反应延迟或人物自己判断。",
            pi_metrics
                .iter()
                .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
                .collect(),
        );
    }

    let clue_metrics = regex_metrics_named(&a.patterns, &["线索面板词"]);
    if !clue_metrics.is_empty() {
        add(
            "P1",
            "信息",
            "线索被面板词收拢",
            "归档、首屏、标签、坐标、重合等词密集时，章节会像任务列表。",
            "检查本章结论是否由场面冲突推出，而不是由屏幕/图表替读者盖章。",
            "把一次完整结论拆成发现、排除、误判、半确认；章末改用行动阻力收束。",
            clue_metrics
                .iter()
                .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
                .collect(),
        );
    }

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
    let negation_count: usize = negation_metrics
        .iter()
        .chain(negation_tokens.iter())
        .map(|m| m.count)
        .sum();
    if !conclusion_metrics.is_empty() || negation_count >= 5 {
        add(
            if !conclusion_metrics.is_empty() {
                "P1"
            } else {
                "P2"
            },
            "句式",
            "否定/肯定判断句过密",
            "不是/只是/而是/这就是一类句子会让旁白替读者解释。",
            "区分人物台词和作者旁白；人物声音可保留，旁白判断优先改。",
            "把结论改成证据、动作或误读；保留一处关键否定，其余让读者自己推出来。",
            conclusion_metrics
                .iter()
                .chain(negation_metrics.iter())
                .chain(negation_tokens.iter())
                .take(4)
                .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
                .collect(),
        );
    }

    let narration_context = a
        .judgement_contexts
        .iter()
        .find(|item| item.context == "narration");
    if let Some(nc) = narration_context {
        if nc.warn {
            add(
                if nc.count >= 5 { "P1" } else { "P2" },
                "句式",
                "旁白判断句偏密",
                "判断词集中在旁白里时，作者会替读者完成理解。",
                "先把人物台词和旁白判断分开；只处理旁白里负责下结论的句子。",
                "把旁白判断改成动作、证据、误读、后果或第三方反应。",
                nc.samples
                    .iter()
                    .take(4)
                    .map(|s| format!("L{} {}：{}", s.line_no, s.terms.join(","), s.text))
                    .collect(),
            );
        }
    }

    let assertive_metrics = regex_metrics_named(&a.patterns, &["肯定判断/解释腔"]);
    let cliche_metrics = regex_metrics_named(
        &a.phrases,
        &["不是因为", "问题不在", "看起来", "更像", "像是", "至少"],
    );
    let cliche_count: usize = assertive_metrics
        .iter()
        .chain(cliche_metrics.iter())
        .map(|m| m.count)
        .sum();
    let cliche_warn =
        assertive_metrics.iter().any(|m| m.warn) || cliche_metrics.iter().any(|m| m.warn);
    if cliche_warn || cliche_count >= 3 {
        add(
            "P2",
            "文风",
            "陈词/解释腔偏密",
            "真正、其实、显然、更像、至少等词会让旁白像评语。",
            "检查这些句子是不是只在解释观感，而没有制造动作或阻力。",
            "删掉只负责解释的句子，或改成角色误读、物件变化、场面后果。",
            assertive_metrics
                .iter()
                .chain(cliche_metrics.iter())
                .take(4)
                .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
                .collect(),
        );
    }

    let sl = &a.sentence_lengths;
    if sl.warn {
        let priority = if sl.short_ratio >= 0.25 || sl.short_runs.len() >= 5 {
            "P1"
        } else {
            "P2"
        };
        let mut evidence = vec![format!(
            "short={}, very_short={}, ratio={}, runs={}",
            sl.short_count,
            sl.very_short_count,
            float_repr(sl.short_ratio),
            sl.short_runs.len()
        )];
        if let Some(first) = sl.short_runs.first() {
            evidence.push(first.sample.join(" | "));
            let roles = format_short_roles(&first.roles, "，");
            if !roles.is_empty() {
                evidence.push(format!("类型：{roles}；建议：{}", first.suggestion));
            }
        }
        add(
            priority,
            "节奏",
            "短句正在变成默认节拍",
            "连续短句会把动作、情绪和信息压成碎拍。",
            "检查短句是在制造节奏，还是在把应展开的过程写成提纲。",
            "每个短句连发块只保留一个节拍点，其余改成动作因果或场面阻力。",
            evidence,
        );
    }

    if !a.fatigue_windows.is_empty() {
        let first_window = &a.fatigue_windows[0];
        add(
            if first_window.score >= 10 { "P1" } else { "P2" },
            "定位",
            "局部句式疲劳窗口",
            "同一小段里短句、判断解释、把字操作或角色起手叠加，会比单项总数更影响观感。",
            "优先看分数最高的窗口，不要平均用力改全章。",
            "保留一个最有用的节奏点；其余改成动作因果、环境反应、人物误读或视角入口。",
            a.fatigue_windows
                .iter()
                .take(3)
                .map(|item| {
                    format!(
                        "S{}-{} L{}-{} score={} {}：{}",
                        item.start_index,
                        item.end_index,
                        item.start_line,
                        item.end_line,
                        item.score,
                        item.reasons.join("、"),
                        item.sample
                            .iter()
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" | ")
                    )
                })
                .collect(),
        );
    }

    let ba_metrics = regex_metrics_named(&a.patterns, &["把字操作句"]);
    let ba_contexts = &a.ba_operation_contexts;
    if !ba_metrics.is_empty() || !ba_contexts.is_empty() {
        let mut evidence: Vec<String> = ba_metrics
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
            evidence.push(line);
        }
        add(
            "P2",
            "动作",
            "把字句过密",
            "把 X 拖上/放进/压住/推过去连续出现时，场面像操作日志。",
            "检查这些把字句属于工具操作、线索操作、情绪动作还是场面调度。",
            "工具操作可保留必要句；线索操作拆发现-误读-后果，情绪动作改身体反应或他人误读。",
            evidence,
        );
    }

    let simile_metrics = regex_metrics_named(&a.patterns, &["像/活像模板"]);
    if !simile_metrics.is_empty() {
        add(
            "P2",
            "文风",
            "比喻模板过密",
            "像/活像类句式能快速给气氛，但过密时会替代真实动作。",
            "检查比喻是否提供新信息；只解释气氛的比喻优先删。",
            "每章保留少数最有新意的比喻，其余改成具体动作、声音、物件变化。",
            simile_metrics
                .iter()
                .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
                .collect(),
        );
    }

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
    if sticky_metrics.iter().map(|m| m.count).sum::<usize>() >= 4
        || sticky_metrics.iter().any(|m| m.warn)
    {
        add(
            "P2",
            "文风",
            "黏糊词/弱判断偏密",
            "轻轻、微微、有点、显得等词会削弱动作力度。",
            "检查这些词是否在替代动作幅度、声音、阻力或人物状态。",
            "优先删弱判断词；用可见动作和场面反应表达轻重。",
            sticky_metrics
                .iter()
                .take(4)
                .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
                .collect(),
        );
    }

    let tracked_warn_list: Vec<&crate::rules::TrackedMetric> =
        a.tracked_terms.iter().filter(|m| m.warn).collect();
    if !tracked_warn_list.is_empty() || !a.tracked_term_windows.is_empty() {
        let mut evidence: Vec<String> = tracked_warn_list
            .iter()
            .take(4)
            .map(|m| metric_evidence(m.count, m.per_10k, &m.samples))
            .collect();
        for item in a.tracked_term_windows.iter().take(3) {
            evidence.push(format!(
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
            "P2",
            "词汇",
            "高频词/点名过密",
            "人物名、地名、设备名过密时，叙述会像点名册或设定表。",
            "检查同一名词是否在局部窗口里连续点名，或是否可以用动作、称谓、空间位置、具体物件轮换。",
            "先改最密的 1-2 个窗口，不要只做同义词替换。",
            evidence,
        );
    }

    let paragraph_leads = &a.paragraph_leads;
    let subject_leads = &a.subject_leads;
    let mut lead_evidence: Vec<String> = Vec::new();
    if !paragraph_leads.is_empty() {
        lead_evidence.push(format!(
            "段首 {}",
            paragraph_leads
                .iter()
                .take(3)
                .map(|i| format!("{} x{}", i.phrase, i.count))
                .collect::<Vec<_>>()
                .join("，")
        ));
    }
    if !subject_leads.is_empty() {
        lead_evidence.push(format!(
            "主语 {}",
            subject_leads
                .iter()
                .take(3)
                .map(|i| format!("{} x{}", i.phrase, i.count))
                .collect::<Vec<_>>()
                .join("，")
        ));
    }
    if !lead_evidence.is_empty() {
        let lead_count = paragraph_leads
            .iter()
            .chain(subject_leads.iter())
            .map(|i| i.count)
            .max()
            .unwrap_or(0);
        if lead_count >= 8 {
            add(
                "P2",
                "镜头",
                "角色名/他她起手过密",
                "段落总从角色名或他她起步，会让镜头调度单一。",
                "检查连续段落是否都是角色先出现、再动作、再判断。",
                "每三到四个角色起手里，至少换一个空间、物件、声音或证据变化起手。",
                lead_evidence,
            );
        }
    }

    let d = &a.dialogue;
    let mut dialogue_evidence: Vec<String> = Vec::new();
    if let Some(first) = d.short_quote_runs.first() {
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
    if !dialogue_evidence.is_empty() {
        add(
            "P2",
            "对话",
            "对白像互答录音",
            "短对白连续互顶时，场面动作会消失。",
            "检查每四句对白里是否有动作、环境变化、第三方打断或设备声作为转轴。",
            "保留最有锋芒的两句，其余用动作、环境声、第三方反应或设备反馈打断。",
            dialogue_evidence,
        );
    }

    let sm = &a.scene_map;
    if sm.warn {
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
            "P2",
            "场面",
            "章节长时间停在同一种功能块",
            "如果整章大部分粗分块都在对白或说明，场面会失去切换和推进。",
            "检查这章有没有让动作、环境、关系和信息交替接力，而不是一直停在解释或接话。",
            "补一个改变站位、空间、外部阻力或关系温度的块，不要只扩写原功能。",
            vec![format!(
                "dominant={} ratio={}；{role_summary}",
                sm.dominant_role,
                float_repr(sm.dominance_ratio)
            )],
        );
    }

    let de = &a.dialogue_emotions;
    if de.flatness_warn || de.volatility_warn {
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
            "P2",
            "对话",
            "对白情绪曲线失衡",
            "对白如果长期只剩逼问/防御一种温度，或情绪标签频繁横跳，人物关系会发假。",
            "检查这段对话是在升级关系、回避真问题，还是只在重复情绪姿态。",
            "给情绪转折补动作、停顿、误读或第三方干扰，让变化落到场面上。",
            vec![format!(
                "dominant={} ratio={} shift={}；{emotion_summary}",
                de.dominant_emotion,
                float_repr(de.dominant_ratio),
                de.shift_count
            )],
        );
    }

    let cv = &a.character_voice;
    if cv.warn {
        let mut evidence = vec![format!(
            "coverage={} dominant={} ratio={}",
            float_repr(cv.coverage_ratio),
            cv.dominant_speaker,
            float_repr(cv.dominant_ratio)
        )];
        for item in cv.speakers.iter().take(3) {
            evidence.push(format!(
                "{} line={} avg={} q={} short={} emotion={}",
                item.speaker,
                item.lines,
                float_repr(item.avg_chars),
                float_repr(item.question_ratio),
                float_repr(item.short_ratio),
                item.dominant_emotion
            ));
        }
        evidence.extend(cv.homogenized_pairs.iter().take(2).cloned());
        add(
            "P2",
            "角色",
            "角色对白开始同腔",
            "多名角色的问句率、短句率、判断姿态和主导情绪过近时，人物声音会并轨。",
            "检查这些角色是不是都在用同一种追问、回避或判断手势说话。",
            "至少给核心角色拉开一项稳定差异：句长、问句密度、脏话/判断句习惯、安抚还是施压入口。",
            evidence,
        );
    }

    let bp = &a.battle_profile;
    if bp.warn {
        add(
            "P2",
            "动作",
            "冲突段动作多但结果少",
            "动作和碰撞已经出现，但结果句、受伤反馈或位移后果不足时，冲突会像挥空。",
            "检查每段动作后，是否有人被逼退、卡住、受伤、失手或改变目标。",
            "每段冲突至少补一个结果句，不要只累计动作动词。",
            vec![format!(
                "sequences={} action={} result={} damage={} ratio={}",
                bp.sequence_count,
                bp.action_hits,
                bp.result_hits,
                bp.damage_hits,
                float_repr(bp.result_ratio)
            )],
        );
    }

    let vp = &a.viewpoint_profile;
    if vp.warn {
        let anchor_summary = if vp.anchor_counts.is_empty() {
            "无".to_string()
        } else {
            vp.anchor_counts
                .iter()
                .map(|(name, count)| format!("{name} x{count}"))
                .collect::<Vec<_>>()
                .join("，")
        };
        let mut evidence = vec![format!(
            "anchors={anchor_summary} switch={} overlap={}",
            vp.switch_count, vp.overlap_count
        )];
        for item in vp.overlaps.iter().take(2) {
            evidence.push(format!(
                "L{} {}：{}",
                item.line_no,
                item.anchors.join(","),
                item.text
            ));
        }
        add(
            "P2",
            "视角",
            "近距离视角锚点漂移",
            "同段多人物心理暴露或近距离切锚偏多时，读者会丢当前镜头中心。",
            "检查这些段落是不是同时替两个人解释内心，或刚贴近一个人就跳去另一个人。",
            "近距离段先固定一个感知中心，其余人物只通过动作、台词和误读出现。",
            evidence,
        );
    }

    let en = &a.ending;
    if en.warn {
        let mut evidence: Vec<String> = Vec::new();
        if !en.flow_terms.is_empty() {
            evidence.push(format!(
                "流程词 {}",
                en.flow_terms
                    .iter()
                    .map(|t| format!("{} x{}", t.term, t.count))
                    .collect::<Vec<_>>()
                    .join("，")
            ));
        }
        if !en.image_terms.is_empty() {
            evidence.push(format!(
                "意象词 {}",
                en.image_terms
                    .iter()
                    .map(|t| format!("{} x{}", t.term, t.count))
                    .collect::<Vec<_>>()
                    .join("，")
            ));
        }
        evidence.push(prefix_chars(&en.tail_excerpt, 120));
        add(
            "P2",
            "章末",
            "章末收束可能模板化",
            "章末反复用冷光、夜、首屏、继续、下一步等词，会让钩子同质。",
            "检查结尾是在打开新行动，还是只把本章线索摆整齐。",
            "在动作余波、关系变化、外部阻力三类里换一种收束手势。",
            evidence,
        );
    }

    let priority_order = |priority: &str| -> i32 {
        match priority {
            "P1" => 0,
            "P2" => 1,
            "P3" => 2,
            _ => 9,
        }
    };
    reminders.sort_by(|a, b| {
        priority_order(&a.priority)
            .cmp(&priority_order(&b.priority))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.title.cmp(&b.title))
    });
    reminders
}
