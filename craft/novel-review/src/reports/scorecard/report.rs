//! 单章记分卡与 story 级 SUMMARY.md 的逐字 markdown 渲染。

use super::*;

use super::axes::{build_consistency_bonus_candidates, ending_display};

/// `recommendation_note`（5 条固定文案）。
fn recommendation_note(recommendation: &str) -> &str {
    match recommendation {
        "retain" => "当前章可保留主体结构，优先微调个别硬项，不要为了清零统计把文气磨平。",
        "light_revise" => {
            "优先处理报告里的硬警告和高优先提醒，重点改局部句法和对白转轴，不必整章推倒。"
        }
        "targeted_rewrite" => {
            "这一章已出现多轴失衡，先按窗口和提醒重写重点段，再回查上游施工图是否诱发了这些问题。"
        }
        "structural_rework" => "需要回退到章节结构或 Story 负载层重做。",
        "rollback_to_plan" => "当前章问题主要来自上游规划，应先修大纲再修正文。",
        other => panic!("未知 recommendation: {other}"),
    }
}

/// `build_scorecard_report`：单章记分卡 markdown（逐字渲染）。
pub fn build_scorecard_report(
    engine: &PlanEngine,
    labels: &EndingLabels,
    draft_path: &Path,
    analysis: &Analysis,
    consistency_snapshot: Option<&StoryConflictSnapshot>,
    story_trend_snapshot: Option<&TrendSnapshot>,
) -> Result<String> {
    let novel_dir = novel_dir_for_draft(draft_path, engine.paths());
    let alignment = match &novel_dir {
        Some(dir) => {
            alignment::build_plan_draft_alignment(engine, draft_path, Some(dir), analysis)?
        }
        None => alignment::Alignment::novel_dir_not_resolved(None),
    };
    let axes = build_axes(
        analysis,
        consistency_snapshot,
        Some(&alignment),
        story_trend_snapshot,
    );
    let (mut gate, mut priority, mut recommendation) = decide_gate(analysis, &axes);
    let convergence_kinds = story_trend_snapshot
        .map(|t| t.convergence_kinds.clone())
        .unwrap_or_default();
    if gate == "PASS" && !convergence_kinds.is_empty() {
        (gate, priority, recommendation) = ("WATCH".into(), "P1".into(), "light_revise".into());
    } else if gate == "WATCH" && convergence_kinds.len() >= 2 && analysis.summary.warn_sections >= 8
    {
        (gate, priority, recommendation) = ("FAIL".into(), "P0".into(), "targeted_rewrite".into());
    }
    let mut bonus_candidates = build_bonus_candidates(analysis);
    bonus_candidates.extend(build_consistency_bonus_candidates(consistency_snapshot));
    let p1_items: Vec<&ReviewReminder> = analysis
        .review_reminders
        .iter()
        .filter(|item| item.priority == "P1")
        .take(6)
        .collect();
    let hard_flags_top: Vec<&HardFlag> = analysis.hard_flags.iter().take(10).collect();
    let p1_count = analysis
        .review_reminders
        .iter()
        .filter(|item| item.priority == "P1")
        .count();

    let stem = draft_path
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("章节文件名必须为合法 UTF-8: {}", draft_path.display()))?;
    let scene_map = &analysis.scene_map;
    let dialogue_emotions = &analysis.dialogue_emotions;
    let tone_profile = &analysis.tone_profile;
    let battle_profile = &analysis.battle_profile;
    let viewpoint_profile = &analysis.viewpoint_profile;
    let character_voice = &analysis.character_voice;
    let bool_str = |flag: bool| if flag { "True" } else { "False" };

    let mut lines: Vec<String> = vec![format!("# {stem} Review Scorecard"), String::new()];
    lines.push(format!("- source: `{}`", draft_path.display()));
    lines.push(format!("- gate: `{gate}`"));
    lines.push(format!("- priority: `{priority}`"));
    lines.push(format!("- recommendation: `{recommendation}`"));
    lines.push(format!("- note: {}", recommendation_note(&recommendation)));
    lines.push(String::new());

    lines.push("## Axis Scores".to_string());
    lines.push("| 维度 | 分数 | 说明 |".to_string());
    lines.push("|---|---:|---|".to_string());
    for axis in &axes {
        lines.push(format!(
            "| {} | `{}` | {} |",
            axis.name, axis.score, axis.reason
        ));
    }
    lines.push(String::new());

    lines.push("## Hard Gates".to_string());
    lines.push(format!(
        "- warn_sections=`{}`",
        analysis.summary.warn_sections
    ));
    lines.push(format!("- hard_flags=`{}`", analysis.hard_flags.len()));
    lines.push(format!("- P1 reminders=`{p1_count}`"));
    lines.push(format!(
        "- fatigue_windows=`{}`",
        analysis.fatigue_window_count
    ));
    lines.push(format!(
        "- tracked_term_windows=`{}`",
        analysis.tracked_term_window_count
    ));
    lines.push(String::new());

    lines.push("## Bonus Candidates".to_string());
    if !bonus_candidates.is_empty() {
        for item in &bonus_candidates {
            lines.push(format!("- `{}`：{}", item.name, item.reason));
        }
    } else {
        lines.push(
            "- 暂无明显可直接保留的设计性重复候选；这不代表没有亮点，只代表脚本暂未捕捉到。"
                .to_string(),
        );
    }
    lines.push(String::new());

    lines.push("## Narrative Signals".to_string());
    lines.push(format!(
        "- scene_blocks=`{}` dominant_role=`{}` dominance_ratio=`{}` switches=`{}`",
        scene_map.block_count,
        scene_map.dominant_role,
        float_repr(scene_map.dominance_ratio),
        scene_map.switch_count
    ));
    lines.push(format!(
        "- dialogue_emotion=`{}` ratio=`{}` shifts=`{}`",
        dialogue_emotions.dominant_emotion,
        float_repr(dialogue_emotions.dominant_ratio),
        dialogue_emotions.shift_count
    ));
    lines.push(format!(
        "- character_voice=`{}` speakers=`{}` coverage=`{}` warn=`{}`",
        if character_voice.dominant_speaker.is_empty() {
            "none"
        } else {
            character_voice.dominant_speaker.as_str()
        },
        character_voice.speaker_count,
        float_repr(character_voice.coverage_ratio),
        bool_str(character_voice.warn)
    ));
    lines.push(format!(
        "- tone=`{}` stable_ratio=`{}` tone_switches=`{}`",
        tone_profile.dominant_tone,
        float_repr(tone_profile.stable_ratio),
        tone_profile.switch_count
    ));
    lines.push(format!(
        "- battle_sequences=`{}` result_ratio=`{}`",
        battle_profile.sequence_count,
        float_repr(battle_profile.result_ratio)
    ));
    lines.push(format!(
        "- viewpoint_anchor=`{}` switches=`{}` overlaps=`{}`",
        if viewpoint_profile.dominant_anchor.is_empty() {
            "none"
        } else {
            viewpoint_profile.dominant_anchor.as_str()
        },
        viewpoint_profile.switch_count,
        viewpoint_profile.overlap_count
    ));
    lines.push(format!(
        "- ending_signal=`{}`",
        ending_display(labels, &infer_ending_label(analysis, labels))
    ));
    if !convergence_kinds.is_empty() {
        lines.push(format!(
            "- trend_convergence: {}",
            convergence_kinds
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if let Some(snapshot) = story_trend_snapshot {
            for note in snapshot.notes.iter().take(3) {
                lines.push(format!("  - `{note}`"));
            }
        }
    }
    lines.push(String::new());

    lines.push("## Consistency Snapshot".to_string());
    match consistency_snapshot {
        Some(snapshot) if snapshot.available => {
            lines.push(format!(
                "- story: `{}`",
                snapshot.story.as_deref().unwrap_or("")
            ));
            lines.push(format!(
                "- feedback_log: `{}`",
                snapshot
                    .feedback_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ));
            lines.push(format!(
                "- confirmed=`{}`",
                snapshot.decision_counter.get("confirmed")
            ));
            lines.push(format!(
                "- false_positive=`{}`",
                snapshot.decision_counter.get("false_positive")
            ));
            lines.push(format!(
                "- designed_keep=`{}`",
                snapshot.decision_counter.get("designed_keep")
            ));
            lines.push(format!(
                "- watch=`{}`",
                snapshot.decision_counter.get("watch")
            ));
            lines.push(format!("- pending=`{}`", snapshot.pending_count));
            lines.push(format!(
                "- review_queue: `{}`",
                snapshot.review_queue_command.as_deref().unwrap_or("")
            ));
            lines.push(format!(
                "- feedback_summary: `{}`",
                snapshot.feedback_summary_command.as_deref().unwrap_or("")
            ));
            if !snapshot.facet_counter.is_empty() {
                lines.push("- facets:".to_string());
                for (name, count) in snapshot.facet_counter.most_common(6) {
                    lines.push(format!("  - `{name}` x{count}"));
                }
            }
            if !snapshot.pending_actions.is_empty() {
                lines.push("- pending actions:".to_string());
                for item in snapshot.pending_actions.iter().take(2) {
                    lines.push(format!(
                        "  - `{}` `{}` confidence=`{}`：{}",
                        item.category, item.title, item.confidence, item.focus
                    ));
                    lines.push(format!("  - command: `{}`", item.command));
                }
            }
        }
        _ => {
            lines.push("- 无一致性反馈快照".to_string());
        }
    }
    lines.push(String::new());

    lines.push("## Plan Alignment Snapshot".to_string());
    if alignment.available {
        lines.push(format!(
            "- chapter_plan: `{}`",
            alignment
                .chapter_plan_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ));
        lines.push(format!(
            "- chapter_function: plan=`{}` draft=`{}` match=`{}`",
            alignment.plan_chapter_function.as_deref().unwrap_or(""),
            alignment.draft_chapter_function.as_deref().unwrap_or(""),
            bool_str(alignment.chapter_match.unwrap_or(false))
        ));
        lines.push(format!(
            "- ending_function: plan=`{}` draft=`{}` match=`{}`",
            alignment.plan_ending_function.as_deref().unwrap_or(""),
            alignment.draft_ending_function.as_deref().unwrap_or(""),
            bool_str(alignment.ending_match.unwrap_or(false))
        ));
        lines.push(format!(
            "- mismatch_count=`{}`",
            alignment.mismatch_count.unwrap_or(0)
        ));
        lines.push(format!(
            "- alignment_status=`{}`",
            alignment.alignment_status.as_deref().unwrap_or("unknown")
        ));
        lines.push(format!(
            "- recommended_action=`{}`",
            alignment
                .recommended_action
                .as_deref()
                .unwrap_or("manual_review")
        ));
        if let Some(drift_types) = &alignment.drift_types {
            if !drift_types.is_empty() {
                lines.push(format!(
                    "- drift_types: {}",
                    drift_types
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        if let Some(review_note) = &alignment.review_note {
            if !review_note.is_empty() {
                lines.push(format!("- note: {review_note}"));
            }
        }
    } else {
        lines.push("- 无 plan-draft 对齐快照".to_string());
    }
    lines.push(String::new());

    lines.push("## Priority Fixes".to_string());
    if !p1_items.is_empty() {
        for item in &p1_items {
            lines.push(format!(
                "- `{}` {}：{} 检查：{} 动作：{}",
                item.category, item.title, item.reason, item.check, item.action
            ));
        }
    } else {
        lines.push("- 无 P1 项".to_string());
    }
    lines.push(String::new());

    lines.push("## Top Hard Flags".to_string());
    if !hard_flags_top.is_empty() {
        for item in &hard_flags_top {
            let mut line = format!(
                "- `{}` `{}` x{}：{}",
                item.section, item.name, item.count, item.note
            );
            if !item.sample.is_empty() {
                line.push_str(&format!(" 样例：{}", item.sample));
            }
            lines.push(line);
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Follow-up".to_string());
    lines.push(
        "- 如果是 `WATCH` 或 `FAIL`，先读对应 `profiles/` 目录里的句式画像，再决定是删词、拆句还是重写段落。".to_string(),
    );
    lines.push(
        "- 如果同一角色、地点、装备或称谓在这章显得摇摆，先跑本章快照里的 `review_queue`，再决定是否补 `feedback-add`。".to_string(),
    );
    lines.push(
        "- 如果 `recommendation` 已接近 `targeted_rewrite`，先回查 `chapter-plan` 和 `story-plan`，不要只在正文层补丁。".to_string(),
    );
    if !convergence_kinds.is_empty() {
        lines.push(
            "- 如果本章处在跨章合流里，优先确认它是不是在重复同一种章末温度，而不是只修单章字词。"
                .to_string(),
        );
    }
    lines.push(String::new());

    Ok(lines.join("\n") + "\n")
}

/// `build_story_summary`：story 级 SUMMARY.md（逐字渲染）。
pub fn build_story_summary(
    story_dir: &Path,
    scorecards: &[(PathBuf, &Analysis)],
    snapshots: &[(PathBuf, StoryConflictSnapshot)],
    labels: &EndingLabels,
    engine: &PlanEngine,
) -> Result<String> {
    let mut ordered: Vec<(PathBuf, &Analysis)> = scorecards.to_vec();
    ordered.sort_by_key(|a| chapter_sort_key(&a.0));
    let story_trend_snapshots = build_story_trend_snapshots(&ordered, labels);
    let mut gate_counter = Counter::default();
    let mut recommendation_counter = Counter::default();
    let mut axis_totals = Counter::default();
    let mut feedback_counter = Counter::default();
    let mut feedback_facets = Counter::default();
    let mut alignment_counter = Counter::default();
    let mut ending_alignment_counter = Counter::default();
    let mut ending_signal_counter = Counter::default();
    let mut ending_signal_flow: Vec<String> = Vec::new();
    let mut story_backlog: Vec<BacklogItem> = Vec::new();

    let mut lines: Vec<String> = vec!["# Review Scorecard Summary".to_string(), String::new()];
    lines.push(format!("- story: `{}`", story_dir.display()));
    lines.push(format!("- chapters: `{}`", ordered.len()));
    lines.push(String::new());

    lines.push("## Chapters".to_string());
    for (path, analysis) in &ordered {
        let snapshot = snapshots.iter().find(|(p, _)| p == path).map(|(_, s)| s);
        let novel_dir = novel_dir_for_draft(path, engine.paths());
        let alignment =
            alignment::build_plan_draft_alignment(engine, path, novel_dir.as_deref(), analysis)?;
        let trend_snapshot = story_trend_snapshots
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, t)| t);
        let axes = build_axes(analysis, snapshot, Some(&alignment), trend_snapshot);
        let (mut gate, mut priority, mut recommendation) = decide_gate(analysis, &axes);
        let convergence_kinds = trend_snapshot
            .map(|t| t.convergence_kinds.clone())
            .unwrap_or_default();
        if gate == "PASS" && !convergence_kinds.is_empty() {
            (gate, priority, recommendation) = ("WATCH".into(), "P1".into(), "light_revise".into());
        } else if gate == "WATCH"
            && convergence_kinds.len() >= 2
            && analysis.summary.warn_sections >= 8
        {
            (gate, priority, recommendation) =
                ("FAIL".into(), "P0".into(), "targeted_rewrite".into());
        }
        gate_counter.add(&gate);
        recommendation_counter.add(&recommendation);
        for axis in &axes {
            axis_totals.add_n(&axis.name, axis.score as usize);
        }
        if let Some(snapshot) = snapshot {
            if snapshot.available {
                for (decision, count) in snapshot.decision_counter.items() {
                    feedback_counter.add_n(&decision, count);
                }
                for (facet, count) in snapshot.facet_counter.items() {
                    feedback_facets.add_n(&facet, count);
                }
                if story_backlog.is_empty() {
                    story_backlog = snapshot.global_feedback_backlog.clone();
                }
            }
        }
        if alignment.available {
            alignment_counter.add_n(
                &format!(
                    "{}->{}",
                    alignment.plan_chapter_function.as_deref().unwrap_or(""),
                    alignment.draft_chapter_function.as_deref().unwrap_or("")
                ),
                1,
            );
            ending_alignment_counter.add_n(
                &format!(
                    "{}->{}",
                    alignment.plan_ending_function.as_deref().unwrap_or(""),
                    alignment.draft_ending_function.as_deref().unwrap_or("")
                ),
                1,
            );
        }
        let ending_signal = infer_ending_label(analysis, labels);
        ending_signal_counter.add(&ending_signal);
        ending_signal_flow.push(ending_signal.clone());
        lines.push(format!(
            "- `{}` gate=`{gate}` priority=`{priority}` recommendation=`{recommendation}` warn_sections=`{}` hard_flags=`{}` ending_signal=`{}`",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(""),
            analysis.summary.warn_sections,
            analysis.hard_flags.len(),
            ending_display(labels, &ending_signal)
        ));
        if let Some(trend_snapshot) = trend_snapshot {
            if !trend_snapshot.notes.is_empty() {
                lines.push(format!(
                    "  - convergence=`{}`",
                    trend_snapshot
                        .notes
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
        }
    }
    lines.push(String::new());

    lines.push("## Gate Distribution".to_string());
    for (name, count) in gate_counter.entries() {
        lines.push(format!("- `{name}` x{count}"));
    }
    lines.push(String::new());

    lines.push("## Recommendation Distribution".to_string());
    for (name, count) in recommendation_counter.entries() {
        lines.push(format!("- `{name}` x{count}"));
    }
    lines.push(String::new());

    lines.push("## Average Axis Scores".to_string());
    for (name, total) in axis_totals.entries() {
        let avg = round2(*total as f64 / ordered.len().max(1) as f64);
        lines.push(format!("- `{name}` avg=`{}`", float_repr(avg)));
    }
    lines.push(String::new());

    lines.push("## Plan-Draft Alignment".to_string());
    if !alignment_counter.is_empty() {
        for (name, count) in alignment_counter.most_common(8) {
            lines.push(format!("- `chapter {name}` x{count}"));
        }
        for (name, count) in ending_alignment_counter.most_common(8) {
            lines.push(format!("- `ending {name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Ending Trend Signals".to_string());
    if !ending_signal_counter.is_empty() {
        for (name, count) in ending_signal_counter.most_common(8) {
            lines.push(format!("- `{}` x{count}", ending_display(labels, &name)));
        }
        lines.push(format!(
            "- flow=`{}`",
            ending_flow_text(&ending_signal_flow, 8, labels)
        ));
        let runs = summarize_runs(&ending_signal_flow, 2, 4, labels);
        let repeated = if runs.is_empty() {
            "无".to_string()
        } else {
            runs.join(" | ")
        };
        lines.push(format!("- repeated=`{repeated}`"));
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Consistency Feedback".to_string());
    if !feedback_counter.is_empty() {
        for decision in ["confirmed", "false_positive", "designed_keep", "watch"] {
            lines.push(format!(
                "- `{decision}` x{}",
                feedback_counter.get(decision)
            ));
        }
        if !feedback_facets.is_empty() {
            lines.push("- facets:".to_string());
            for (name, count) in feedback_facets.most_common(8) {
                lines.push(format!("  - `{name}` x{count}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Consistency Backlog".to_string());
    if !story_backlog.is_empty() {
        for item in story_backlog.iter().take(6) {
            lines.push(format!("- `{}` {}", item.target, item.reason));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    Ok(lines.join("\n") + "\n")
}
