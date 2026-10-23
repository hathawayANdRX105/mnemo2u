//! AUDIT.md 看板渲染与 catalog 标量渲染（输出逐字节稳定）。

use super::*;

use super::trajectories::build_narrative_trajectory_rows;

/// 标量渲染（catalog JSON 值 → str）：int 十进制 / str 原样 / None → "None"。
fn json_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// 渲染单份 AUDIT.md 看板（输出逐字节稳定）。
pub(crate) fn build_dashboard(
    novel_dir: &Path,
    concept: &ConceptSection,
    plans: &PlanSection,
    drafts: &DraftSection,
    consistency: &ConsistencySection,
) -> String {
    let narrative_trajectories = build_narrative_trajectory_rows(drafts, consistency);
    let novel_name = novel_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut lines: Vec<String> = vec!["# AUDIT DASHBOARD".to_string(), String::new()];
    lines.push(format!("- novel: `{novel_name}`"));
    lines.push(format!(
        "- concept cards: `{}`",
        if concept.exists { concept.files } else { 0 }
    ));
    lines.push(format!(
        "- plan files: `{}`",
        if plans.exists { plans.files } else { 0 }
    ));
    lines.push(format!(
        "- draft chapters: `{}`",
        if drafts.exists { drafts.files } else { 0 }
    ));
    lines.push(String::new());

    // ## Concept
    lines.push("## Concept".to_string());
    if !concept.exists {
        lines.push("- 无概念卡目录".to_string());
    } else {
        lines.push(format!("- total warnings: `{}`", concept.warnings));
        for item in &concept.categories {
            lines.push(format!(
                "- `{}` cards=`{}` warnings=`{}` top=`{}` (`{}` | {})",
                item.category,
                item.cards,
                item.warnings,
                item.top_file,
                item.top_count,
                item.top_kinds
            ));
        }
        lines.push("- summary files:".to_string());
        for path in &concept.summary_paths {
            lines.push(format!("  - `{}`", path.display()));
        }
    }
    lines.push(String::new());

    // ## Plans
    lines.push("## Plans".to_string());
    if !plans.exists {
        lines.push("- 无大纲目录".to_string());
    } else {
        lines.push(format!("- total warnings: `{}`", plans.warnings));
        lines.push(format!(
            "- chapter functions: {}",
            plans.chapter_function_distribution
        ));
        lines.push(format!(
            "- ending functions: {}",
            plans.ending_function_distribution
        ));
        if !plans.story_trends.is_empty() {
            lines.push("- story function trends:".to_string());
            for item in plans.story_trends.iter().take(8) {
                lines.push(format!(
                    "  - `{}` chapter_flow=`{}` ending_flow=`{}`",
                    item.story, item.chapter_flow, item.ending_flow
                ));
                if !item.chapter_runs.is_empty() || !item.ending_runs.is_empty() {
                    let mut run_parts: Vec<String> = Vec::new();
                    if !item.chapter_runs.is_empty() {
                        run_parts.push(format!("chapter_runs={}", item.chapter_runs.join(" | ")));
                    }
                    if !item.ending_runs.is_empty() {
                        run_parts.push(format!("ending_runs={}", item.ending_runs.join(" | ")));
                    }
                    lines.push(format!("    trend=`{}`", run_parts.join("; ")));
                }
            }
        }
        for item in &plans.plan_types {
            lines.push(format!(
                "- `{}` files=`{}` warnings=`{}` top=`{}` (`{}` | {})",
                item.plan_type,
                item.files,
                item.warnings,
                item.top_file,
                item.top_count,
                item.top_kinds
            ));
        }
        lines.push("- summary files:".to_string());
        for path in &plans.summary_paths {
            lines.push(format!("  - `{}`", path.display()));
        }
    }
    lines.push(String::new());

    // ## Drafts
    lines.push("## Drafts".to_string());
    if !drafts.exists {
        lines.push("- 无草稿目录".to_string());
    } else {
        lines.push(format!("- total warn sections: `{}`", drafts.warnings));
        lines.push(format!(
            "- workspace templates: {}",
            drafts.workspace_templates
        ));
        lines.push(format!("- deposition targets: {}", drafts.template_targets));
        lines.push(format!(
            "- plan-draft alignment: {}",
            drafts.alignment_summary
        ));
        for item in &drafts.stories {
            lines.push(format!(
                "- `{}` chapters=`{}` warn_sections=`{}` top=`{}` (`{}` | {}) fatigue=`{}`",
                item.story,
                item.chapters,
                item.warnings,
                item.top_file,
                item.top_count,
                item.top_templates,
                item.top_fatigue
            ));
            lines.push(format!(
                "  gate=`{}` recommendation=`{}` avg_axes=`{}`",
                item.gate_summary, item.recommendation_summary, item.avg_axis
            ));
            lines.push(format!(
                "  narrative=`scene:{} tone:{} emotion:{} speakers:{}` templates=`{}` alignment=`{}` ending_signals=`{}`",
                item.scene_summary, item.tone_summary, item.emotion_summary, item.speaker_summary,
                item.template_summary, item.alignment_summary, item.ending_signal_summary
            ));
            if !item.alignment_mismatches.is_empty() {
                lines.push(format!(
                    "  drift=`{}`",
                    item.alignment_mismatches
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
            if !item.voice_drifts.is_empty() {
                lines.push(format!(
                    "  voice=`{}`",
                    item.voice_drifts
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
            let has_trend = !item.chapter_runs.is_empty()
                || !item.ending_runs.is_empty()
                || !item.ending_signal_runs.is_empty()
                || !item.tone_runs.is_empty()
                || !item.emotion_runs.is_empty()
                || !item.trend_convergences.is_empty();
            if has_trend {
                let mut trend_parts: Vec<String> = Vec::new();
                if !item.chapter_runs.is_empty() {
                    trend_parts.push(format!("chapter_runs={}", item.chapter_runs.join(" | ")));
                }
                if !item.ending_runs.is_empty() {
                    trend_parts.push(format!("ending_runs={}", item.ending_runs.join(" | ")));
                }
                if !item.ending_signal_runs.is_empty() {
                    trend_parts.push(format!(
                        "ending_signal_runs={}",
                        item.ending_signal_runs.join(" | ")
                    ));
                }
                if !item.trend_convergences.is_empty() {
                    trend_parts.push(format!(
                        "convergence={}",
                        item.trend_convergences.join(" | ")
                    ));
                }
                if !item.tone_runs.is_empty() {
                    trend_parts.push(format!("tone_runs={}", item.tone_runs.join(" | ")));
                }
                if !item.emotion_runs.is_empty() {
                    trend_parts.push(format!("emotion_runs={}", item.emotion_runs.join(" | ")));
                }
                lines.push(format!("  trend=`{}`", trend_parts.join("; ")));
                lines.push(format!(
                    "  ending_signal_flow=`{}`",
                    item.ending_signal_flow
                ));
            }
        }
        lines.push("- mirror stats:".to_string());
        lines.push(format!("  - `{}`", novel_dir.join("draft-stats").display()));
        lines.push("- scorecard summaries:".to_string());
        for item in &drafts.stories {
            lines.push(format!("  - `{}`", item.scorecard_summary_path.display()));
        }
        lines.push("- review kits:".to_string());
        for item in &drafts.stories {
            lines.push(format!("  - `{}`", item.review_kit_summary_path.display()));
        }
        lines.push("- template backlogs:".to_string());
        for item in &drafts.stories {
            lines.push(format!(
                "  - `{}`",
                item.template_backlog_summary_path.display()
            ));
        }
        lines.push("- template research:".to_string());
        lines.push(format!("  - `{}`", drafts.template_research_path.display()));
        lines.push("- template catalog:".to_string());
        lines.push(format!(
            "  - `{}`",
            drafts.template_catalog_summary_path.display()
        ));
        lines.push(format!(
            "  - `{}`",
            drafts.template_catalog_json_path.display()
        ));
        if !drafts.template_learning_anchors.is_empty() {
            lines.push("- learning anchors:".to_string());
            for item in &drafts.template_learning_anchors {
                let kind = json_scalar(item.get("kind").unwrap_or(&Value::Null));
                let bucket_name = format!(
                    "{}::{}",
                    json_scalar(item.get("bucket").unwrap_or(&Value::Null)),
                    json_scalar(item.get("name").unwrap_or(&Value::Null))
                );
                let story_count = json_scalar(item.get("story_count").unwrap_or(&Value::Null));
                let count = json_scalar(item.get("count").unwrap_or(&Value::Null));
                lines.push(format!(
                    "  - `{kind}` `{bucket_name}` stories=`{story_count}` total=`{count}`"
                ));
            }
        }
        if !drafts.template_writeback_queue.is_empty() {
            lines.push("- writeback queue:".to_string());
            for item in &drafts.template_writeback_queue {
                lines.push(format!(
                    "  - `{}` `{}` stories=`{}` total=`{}` -> `{}`",
                    json_scalar(item.get("kind").unwrap_or(&Value::Null)),
                    json_scalar(item.get("name").unwrap_or(&Value::Null)),
                    json_scalar(item.get("stories").unwrap_or(&Value::Null)),
                    json_scalar(item.get("count").unwrap_or(&Value::Null)),
                    json_scalar(item.get("target").unwrap_or(&Value::Null))
                ));
            }
        }
        if !drafts.alignment_mismatches.is_empty() {
            lines.push("- plan-draft drift samples:".to_string());
            for item in &drafts.alignment_mismatches {
                lines.push(format!(
                    "  - `{}` chapter=`{}` ending=`{}`",
                    item.draft, item.chapter, item.ending
                ));
            }
        }
    }
    lines.push(String::new());

    // ## Consistency
    lines.push("## Consistency".to_string());
    lines.push(format!("- index: `{}`", consistency.db_path.display()));
    lines.push(format!(
        "- feedback log: `{}` entries=`{}`",
        consistency.feedback_path.display(),
        consistency.feedback_entries
    ));
    if !consistency.feedback_decisions.is_empty() {
        lines.push(format!(
            "- feedback decisions: {}",
            consistency
                .feedback_decisions
                .iter()
                .map(|(name, count)| format!("`{name}` x{count}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    } else {
        lines.push("- feedback decisions: 无".to_string());
    }
    if !consistency.feedback_facets.is_empty() {
        lines.push(format!(
            "- feedback facets: {}",
            consistency
                .feedback_facets
                .iter()
                .map(|(name, count)| format!("`{name}` x{count}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    } else {
        lines.push("- feedback facets: 无".to_string());
    }
    lines.push(format!(
        "- pending review: `{}`",
        consistency.feedback_pending
    ));
    if !consistency.feedback_backlog.is_empty() {
        lines.push("- feedback backlog:".to_string());
        for item in consistency.feedback_backlog.iter().take(4) {
            lines.push(format!("  - `{}` {}", item.target, item.reason));
        }
    } else {
        lines.push("- feedback backlog: 无".to_string());
    }
    if !consistency.feedback_pending_samples.is_empty() {
        lines.push("- pending samples:".to_string());
        for row in &consistency.feedback_pending_samples {
            lines.push(format!(
                "  - `{}` `{}` `{}` confidence=`{}`",
                row.story, row.category, row.title, row.confidence
            ));
        }
    } else {
        lines.push("- pending samples: 无".to_string());
    }
    if !consistency.story_alignment.is_empty() {
        lines.push("- story alignment:".to_string());
        for item in consistency.story_alignment.iter().take(12) {
            let mut line = format!(
                "  - `{}` plan_entities=`{}` draft_entities=`{}`",
                item.story, item.plan_entities, item.draft_entities
            );
            if let Some(only) = &item.plan_only_entities {
                if !only.is_empty() {
                    line.push_str(&format!(" plan_only=`{only}`"));
                }
            }
            if let Some(only) = &item.draft_only_entities {
                if !only.is_empty() {
                    line.push_str(&format!(" draft_only=`{only}`"));
                }
            }
            lines.push(line);
        }
    } else {
        lines.push("- story alignment: 无".to_string());
    }
    if !narrative_trajectories.is_empty() {
        lines.push("- narrative trajectories:".to_string());
        for item in narrative_trajectories.iter().take(8) {
            lines.push(format!("  - `{}` {}", item.story, item.summary));
            if !item.details.is_empty() {
                lines.push(format!(
                    "    detail=`{}`",
                    item.details
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
        }
    } else {
        lines.push("- narrative trajectories: 无".to_string());
    }
    if !consistency.story_trajectories.is_empty() {
        lines.push("- story trajectories:".to_string());
        for item in consistency.story_trajectories.iter().take(8) {
            lines.push(format!(
                "  - `{}` state=`{}` goal=`{}` relationship=`{}`",
                item.story, item.state_summary, item.goal_summary, item.relationship_summary
            ));
            if !item.samples.is_empty() {
                lines.push(format!(
                    "    sample=`{}`",
                    item.samples
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
        }
    } else {
        lines.push("- story trajectories: 无".to_string());
    }
    if !consistency.story_trajectory_details.is_empty() {
        lines.push("- trajectory details:".to_string());
        for item in consistency.story_trajectory_details.iter().take(6) {
            lines.push(format!("  - `{}`", item.story));
            for detail in item.items.iter().take(4) {
                let mut line = format!(
                    "    - `{}` `{}` {}",
                    detail.kind, detail.title, detail.summary
                );
                if !detail.timeline.is_empty() {
                    line.push_str(&format!(
                        " timeline=`{}`",
                        detail
                            .timeline
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ));
                }
                lines.push(line);
            }
        }
    } else {
        lines.push("- trajectory details: 无".to_string());
    }
    if !consistency.relationship_pair_trajectories.is_empty() {
        lines.push("- relationship pair trajectories:".to_string());
        for item in consistency.relationship_pair_trajectories.iter().take(6) {
            lines.push(format!("  - `{}`", item.story));
            for detail in item.items.iter().take(4) {
                let mut line = format!("    - `{}` {}", detail.pair, detail.summary);
                if !detail.timeline.is_empty() {
                    line.push_str(&format!(
                        " timeline=`{}`",
                        detail
                            .timeline
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ));
                }
                lines.push(line);
            }
        }
    } else {
        lines.push("- relationship pair trajectories: 无".to_string());
    }
    if !consistency.story_tension.is_empty() {
        lines.push("- state tension:".to_string());
        for item in consistency.story_tension.iter().take(12) {
            let mut parts: Vec<String> = Vec::new();
            let inj_n = item.injury_negative.as_deref().unwrap_or("");
            let inj_s = item.injury_stable.as_deref().unwrap_or("");
            if !inj_n.is_empty() && !inj_s.is_empty() {
                parts.push(format!("injury={inj_n} -> {inj_s}"));
            }
            let eq_d = item.equipment_damaged.as_deref().unwrap_or("");
            let eq_a = item.equipment_active.as_deref().unwrap_or("");
            if !eq_d.is_empty() && !eq_a.is_empty() {
                parts.push(format!("equipment={eq_d} -> {eq_a}"));
            }
            lines.push(format!(
                "  - `{}` `{}` ({}) {}",
                item.story,
                item.title,
                item.category,
                parts.join(" ; ")
            ));
        }
    } else {
        lines.push("- state tension: 无".to_string());
    }
    if !consistency.story_goal_tension.is_empty() {
        lines.push("- goal tension:".to_string());
        for item in consistency.story_goal_tension.iter().take(12) {
            let mut parts: Vec<String> = Vec::new();
            let g = item.goal_assigned.as_deref().unwrap_or("");
            let c = item.goal_changed.as_deref().unwrap_or("");
            let done = item.goal_completed.as_deref().unwrap_or("");
            if !g.is_empty() {
                parts.push(format!("assigned={g}"));
            }
            if !c.is_empty() {
                parts.push(format!("changed={c}"));
            }
            if !done.is_empty() {
                parts.push(format!("completed={done}"));
            }
            lines.push(format!(
                "  - `{}` `{}` ({}) {}",
                item.story,
                item.title,
                item.category,
                parts.join(" ; ")
            ));
        }
    } else {
        lines.push("- goal tension: 无".to_string());
    }
    if !consistency.story_relationship_tension.is_empty() {
        lines.push("- relationship tension:".to_string());
        for item in consistency.story_relationship_tension.iter().take(12) {
            lines.push(format!(
                "  - `{}` `{}` ({}) close={} ; distant={}",
                item.story,
                item.title,
                item.category,
                item.relationship_close.as_deref().unwrap_or(""),
                item.relationship_distant.as_deref().unwrap_or("")
            ));
        }
    } else {
        lines.push("- relationship tension: 无".to_string());
    }
    if !consistency.story_conflicts.is_empty() {
        lines.push("- conflict candidates:".to_string());
        for item in consistency.story_conflicts.iter().take(12) {
            lines.push(format!(
                "  - `{}` `{}` `{}` ({}) confidence=`{}` support=`{}` {}",
                item.story,
                item.category,
                item.title,
                item.entity_category,
                item.confidence,
                item.support_note,
                item.summary
            ));
        }
    } else {
        lines.push("- conflict candidates: 无".to_string());
    }
    lines.push(String::new());

    // ## Suggested Order（逐字照抄）
    lines.push("## Suggested Order".to_string());
    lines.push("1. 先修 `draft` 里 `gate=FAIL`、`recommendation=targeted_rewrite` 的章节，再看 `pairs / triples`".to_string());
    lines.push("2. 再修 `chapter-plan` 与 `story-plan` 的字段错位和空字段".to_string());
    lines.push("3. 如果某章评分里 `一致性准备度` 明显偏低，先跑 `novel-review consistency suspects` 再决定是否只是局部误写".to_string());
    lines.push("4. 如果已经锁定某条 Story，要逐条复核一致性候选，直接跑 `novel-review consistency review-queue novel1 --story storyN`".to_string());
    lines.push("5. 做完一轮局部复核后，立刻跑 `novel-review consistency feedback-summary novel1 --story storyN` 看这一条 Story 是否开始收敛".to_string());
    lines.push("6. 最后补 `concept` 缺口，避免下游继续空转".to_string());
    lines.join("\n") + "\n"
}
