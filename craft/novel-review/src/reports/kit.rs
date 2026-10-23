//! 故事级评审套件（`reports-kit` 子命令）。
//!
//! 每章写 scorecards / learning / profiles 三类单章报告，story 级写四个
//! SUMMARY + 模板 backlog 两件套 + `review-kit/SUMMARY.md`（字节稳定渲染）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::draft::{analyze_path, build_corpus_profile, Analysis, DraftContext};
use crate::audit::plan::PlanEngine;
use crate::config::{self, EndingLabels};
use crate::consistency::{build_story_conflict_snapshot_from_path, StoryConflictSnapshot};
use crate::input::{write_json, write_text};
use crate::reports::alignment;
use crate::reports::backlog;
use crate::reports::learning;
use crate::reports::profiles;
use crate::reports::scorecard;
use crate::rules::build_template_bank;
use crate::stats::draft::{
    chapter_sort_key, collect_chapter_files, ending_display, infer_ending_label, stats_path_for,
    Ctr,
};

/// `reports-kit` 子命令参数（位置 `paths` nargs+、
/// `--sample-limit` 缺省 6）。
#[derive(Debug, Clone)]
pub struct KitOptions {
    /// 产物根目录；缺省写在输入树旁。
    pub output_root: Option<PathBuf>,
    /// 草稿章文件或目录。
    pub paths: Vec<PathBuf>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// 每条规则最多记录的样本行数（默认 6）。
    pub sample_limit: usize,
}

/// `review_kit_path_for(draft)`：`stats_path_for(draft).parent / review-kit / SUMMARY.md`。
pub fn review_kit_path_for(
    draft_path: &Path,
    paths: &crate::config::PathsConfig,
    output_root: Option<&Path>,
) -> Result<PathBuf> {
    let stats = stats_path_for(draft_path, output_root, paths)?;
    Ok(stats
        .parent()
        .context("stats 路径无父目录")?
        .join("review-kit")
        .join("SUMMARY.md"))
}

/// 派单条目（`add_assignment` 键序）。
#[derive(Debug, Clone)]
pub struct Assignment {
    pub priority: String,
    pub chapter: String,
    pub title: String,
    pub reason: String,
    pub action: String,
    pub source: String,
}

/// `assignment_key`：`(priority, chapter, title)`。
fn assignment_key(item: &Assignment) -> (String, String, String) {
    (
        item.priority.clone(),
        item.chapter.clone(),
        item.title.clone(),
    )
}

/// `assignment_sort_key`：`(priority_rank, source_rank, chapter, title)`。
fn assignment_sort_key(item: &Assignment) -> (i32, i32, String, String) {
    let priority_rank = match item.priority.as_str() {
        "P0" => 0,
        "P1" => 1,
        "P2" => 2,
        _ => 3,
    };
    let source_rank = match item.source.as_str() {
        "scorecards" => 0,
        "plan_draft_alignment" => 1,
        "ending_trends" => 2,
        "trend_convergence" => 3,
        "consistency_index" => 4,
        "template_backlog" => 5,
        _ => 5,
    };
    (
        priority_rank,
        source_rank,
        item.chapter.clone(),
        item.title.clone(),
    )
}

/// `add_assignment`（按 `(priority, chapter, title)` 首现去重）。
fn add_assignment(assignments: &mut Vec<Assignment>, item: Assignment) {
    let key = assignment_key(&item);
    if assignments
        .iter()
        .map(assignment_key)
        .any(|existing| existing == key)
    {
        return;
    }
    assignments.push(item);
}

/// 派单条目（`collect_review_assignments` 内各 `add_assignment` 调用）。
fn make_assignment(
    priority: &str,
    chapter: &str,
    title: String,
    reason: String,
    action: &str,
    source: &str,
) -> Assignment {
    Assignment {
        priority: priority.to_string(),
        chapter: chapter.to_string(),
        title,
        reason,
        action: action.to_string(),
        source: source.to_string(),
    }
}

/// 连续值游程（label/value 连续相同且长度 >= 2 的章序列）。
struct Run {
    label: String,
    paths: Vec<PathBuf>,
}

/// 值游程（`_collect_repeated_value_runs`，缺省 `min_run=2`）。
fn collect_repeated_value_runs(rows: &[(PathBuf, String)]) -> Vec<(String, Vec<PathBuf>)> {
    let mut runs: Vec<(String, Vec<PathBuf>)> = Vec::new();
    let mut ordered: Vec<&(PathBuf, String)> = rows.iter().collect();
    ordered.sort_by_key(|(path, _)| chapter_sort_key(path));

    let mut current_value: Option<String> = None;
    let mut current_paths: Vec<PathBuf> = Vec::new();

    let flush = |runs: &mut Vec<(String, Vec<PathBuf>)>,
                 current_value: &mut Option<String>,
                 current_paths: &mut Vec<PathBuf>| {
        if current_value.is_some() && current_paths.len() >= 2 {
            runs.push((
                current_value.clone().unwrap_or_default(),
                current_paths.clone(),
            ));
        }
        *current_value = None;
        current_paths.clear();
    };

    for (path, value) in ordered {
        let value = value.as_str();
        if current_value.as_deref() == Some(value) {
            current_paths.push(path.clone());
            continue;
        }
        flush(&mut runs, &mut current_value, &mut current_paths);
        current_value = Some(value.to_string());
        current_paths = vec![path.clone()];
    }
    flush(&mut runs, &mut current_value, &mut current_paths);
    runs
}

/// 章末标签连续段（chapter 序，连续同 label 且 >= 2）。
fn collect_repeated_ending_runs(
    analyses: &[(PathBuf, &Analysis)],
    labels: &EndingLabels,
) -> Vec<Run> {
    let rows: Vec<(PathBuf, String)> = analyses
        .iter()
        .map(|(path, analysis)| (path.clone(), infer_ending_label(analysis, labels)))
        .collect();
    collect_repeated_value_runs(&rows)
        .into_iter()
        .map(|(label, paths)| Run { label, paths })
        .collect()
}

/// 收敛趋势段。
fn collect_converging_trend_runs(
    analyses: &[(PathBuf, &Analysis)],
    labels: &EndingLabels,
) -> Vec<ConvergeRun> {
    let ending_runs = collect_repeated_ending_runs(analyses, labels);
    let tone_rows: Vec<(PathBuf, String)> = analyses
        .iter()
        .map(|(path, analysis)| {
            let tone = analysis.tone_profile.dominant_tone.as_str();
            let tone = if tone.is_empty() { "none" } else { tone };
            (path.clone(), tone.to_string())
        })
        .collect();
    let emotion_rows: Vec<(PathBuf, String)> = analyses
        .iter()
        .map(|(path, analysis)| {
            let emotion = analysis.dialogue_emotions.dominant_emotion.as_str();
            let emotion = if emotion.is_empty() {
                "neutral"
            } else {
                emotion
            };
            (path.clone(), emotion.to_string())
        })
        .collect();
    let tone_runs = collect_repeated_value_runs(&tone_rows);
    let emotion_runs = collect_repeated_value_runs(&emotion_rows);

    let mut convergences: Vec<ConvergeRun> = Vec::new();
    for ending_run in &ending_runs {
        let ending_paths: std::collections::HashSet<&PathBuf> = ending_run.paths.iter().collect();
        let ending_label = ending_display(labels, &ending_run.label);
        for (tone, tone_paths) in &tone_runs {
            if *tone == "none" {
                continue;
            }
            let overlap: Vec<PathBuf> = tone_paths
                .iter()
                .filter(|path| ending_paths.contains(*path))
                .cloned()
                .collect();
            if overlap.len() >= 2 {
                convergences.push(ConvergeRun {
                    kind: "ending_tone".to_string(),
                    paths: overlap,
                    ending_label: ending_label.clone(),
                    secondary_label: tone.clone(),
                });
            }
        }
        for (emotion, emotion_paths) in &emotion_runs {
            if *emotion == "neutral" {
                continue;
            }
            let overlap: Vec<PathBuf> = emotion_paths
                .iter()
                .filter(|path| ending_paths.contains(*path))
                .cloned()
                .collect();
            if overlap.len() >= 2 {
                convergences.push(ConvergeRun {
                    kind: "ending_emotion".to_string(),
                    paths: overlap,
                    ending_label: ending_label.clone(),
                    secondary_label: emotion.clone(),
                });
            }
        }
    }
    convergences
}

/// 合流游程（`ending_tone` / `ending_emotion`）。
struct ConvergeRun {
    kind: String,
    paths: Vec<PathBuf>,
    ending_label: String,
    secondary_label: String,
}

/// 评审套件派单（`ordered` 为章文件 ×
/// 分析，`snapshots` 为章文件 × 一致性快照，均按输入序）。
pub fn collect_review_assignments(
    story_dir: &Path,
    ordered: &[(PathBuf, &Analysis)],
    snapshots: &[(PathBuf, StoryConflictSnapshot)],
    labels: &EndingLabels,
    engine: &PlanEngine,
) -> Result<Vec<Assignment>> {
    let mut assignments: Vec<Assignment> = Vec::new();
    let mut template_counter: Ctr = Ctr::default();
    let mut consistency_added = false;

    for (draft_path, analysis) in ordered {
        let chapter = draft_path
            .file_stem()
            .and_then(|s| s.to_str())
            .with_context(|| format!("章节文件名必须为合法 UTF-8: {}", draft_path.display()))?;
        let snapshot = snapshots
            .iter()
            .find(|(p, _)| p == draft_path)
            .map(|(_, s)| s);
        let novel_dir = scorecard::novel_dir_for_draft(draft_path, engine.paths());
        let align = match &novel_dir {
            Some(dir) => {
                alignment::build_plan_draft_alignment(engine, draft_path, Some(dir), analysis)?
            }
            None => alignment::Alignment::novel_dir_not_resolved(None),
        };
        let axes = scorecard::build_axes(analysis, snapshot, Some(&align), None);
        let (gate, priority, recommendation) = scorecard::decide_gate(analysis, &axes);

        if gate == "WATCH" || gate == "FAIL" {
            let p = if gate == "FAIL" { "P1" } else { &priority };
            add_assignment(
                &mut assignments,
                make_assignment(
                    p,
                    chapter,
                    format!("复核 `{chapter}` 的 {gate} scorecard"),
                    format!(
                        "gate=`{gate}` recommendation=`{recommendation}` warn_sections=`{}` hard_flags=`{}`",
                        analysis.summary.warn_sections,
                        analysis.hard_flags.len(),
                    ),
                    "先读 `scorecards/` 对应章节，再按 P1 reminders 和 hard flags 定位局部重写点。",
                    "scorecards",
                ),
            );
        }

        for reminder in &analysis.review_reminders {
            if !matches!(reminder.priority.as_str(), "P1" | "P2") {
                continue;
            }
            if reminder.priority == "P2" && assignments.len() >= 10 {
                continue;
            }
            let action = &reminder.action;
            add_assignment(
                &mut assignments,
                make_assignment(
                    &reminder.priority,
                    chapter,
                    format!("复核 `{chapter}`：{}", reminder.title),
                    reminder.reason.clone(),
                    action,
                    &format!("review_reminders/{}", reminder.category),
                ),
            );
        }

        if align.available && align.mismatch_count.unwrap_or(0) >= 1 {
            add_assignment(
                &mut assignments,
                make_assignment(
                    "P1",
                    chapter,
                    format!("复核 `{chapter}` 的 plan-draft 漂移"),
                    format!(
                        "status=`{status}` action=`{action}`；chapter `{plan_ch}`→`{draft_ch}`；ending `{plan_end}`→`{draft_end}`",
                        status = align
                            .alignment_status
                            .as_deref()
                            .unwrap_or("unknown"),
                        action = align
                            .recommended_action
                            .as_deref()
                            .unwrap_or("manual_review"),
                        plan_ch = align
                            .plan_chapter_function
                            .as_deref()
                            .unwrap_or("None"),
                        draft_ch = align
                            .draft_chapter_function
                            .as_deref()
                            .unwrap_or("None"),
                        plan_end = align
                            .plan_ending_function
                            .as_deref()
                            .unwrap_or("None"),
                        draft_end = align
                            .draft_ending_function
                            .as_deref()
                            .unwrap_or("None"),
                    ),
                    align
                        .review_note
                        .as_deref()
                        .unwrap_or("先判断该修正文落点，还是回修 chapter-plan / story-plan；不要只当文风问题处理。"),
                    "plan_draft_alignment",
                ),
            );
        }

        if let Some(snap) = snapshot {
            if snap.available && !snap.pending_rows.is_empty() && !consistency_added {
                let pending_count = snap.pending_rows.len();
                let story = snap
                    .story
                    .as_deref()
                    .unwrap_or(story_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                add_assignment(
                    &mut assignments,
                    make_assignment(
                        "P1",
                        story,
                        format!("复核 `{story}` 的一致性 pending"),
                        format!("当前一致性快照仍有 `{pending_count}` 条候选未判定。"),
                        &format!(
                            "跑 `{}`，复核后用 `feedback-add` 写回判定。",
                            snap.review_queue_command.as_deref().unwrap_or("")
                        ),
                        "consistency_index",
                    ),
                );
                consistency_added = true;
            }
        }

        for item in learning::collect_template_backlog(analysis) {
            template_counter.add(&format!("{}::{}", item.bucket, item.name), 1);
        }
    }

    for run in collect_repeated_ending_runs(ordered, labels) {
        let paths: Vec<String> = run
            .paths
            .iter()
            .map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let chapter_span = if paths.len() >= 2 {
            format!("{}-{}", paths[0], paths[paths.len() - 1])
        } else {
            paths[0].clone()
        };
        let ending_label = ending_display(labels, &run.label);
        let priority = if run.paths.len() >= 3 { "P1" } else { "P2" };
        add_assignment(
            &mut assignments,
            make_assignment(
                priority,
                story_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                format!("复核 `{chapter_span}` 的同类章末连发"),
                format!(
                    "连续 `{}` 章落在 `{ending_label}`，跨章读感可能开始同质化。",
                    run.paths.len(),
                ),
                "对照 pairs / triples 的 `endings=` 和 `repeated=`，确认这些结尾是在推进不同后果，还是只是在重复同一种收束手势。",
                "ending_trends",
            ),
        );
    }

    for item in collect_converging_trend_runs(ordered, labels) {
        let paths: Vec<String> = item
            .paths
            .iter()
            .map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let chapter_span = if paths.len() >= 2 {
            format!("{}-{}", paths[0], paths[paths.len() - 1])
        } else {
            paths[0].clone()
        };
        let (title, reason, action) = if item.kind == "ending_tone" {
            (
                format!("复核 `{chapter_span}` 的章末-色调合流"),
                format!(
                    "连续 `{}` 章同时落在章末 `{}` 和色调 `{}`，跨章读感可能开始同温度、同收束。",
                    item.paths.len(),
                    item.ending_label,
                    item.secondary_label,
                ),
                "先看 scorecards / profiles 的 tone 与 ending trend，再判断这些章节是在持续累积压迫，还是已经写成同一种章末氛围模板。",
            )
        } else {
            (
                format!("复核 `{chapter_span}` 的章末-对白情绪合流"),
                format!(
                    "连续 `{}` 章同时落在章末 `{}` 和对白情绪 `{}`，跨章关系推进可能开始同质化。",
                    item.paths.len(),
                    item.ending_label,
                    item.secondary_label,
                ),
                "先看 learning / profiles 的 dialogue emotion 与 ending trend，再判断这些结尾是不是一直在用同一种情绪温度收束关系。",
            )
        };
        let priority = if item.paths.len() >= 3 { "P1" } else { "P2" };
        add_assignment(
            &mut assignments,
            make_assignment(
                priority,
                story_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                title,
                reason,
                action,
                "trend_convergence",
            ),
        );
    }

    for (name, count) in template_counter.most_common(4) {
        if count < 2 {
            continue;
        }
        add_assignment(
            &mut assignments,
            make_assignment(
                "P2",
                story_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                format!("判断跨章模板候选 `{name}` 是否该沉淀"),
                format!(
                    "该候选在本 Story 中出现 `{count}` 次，已经值得人工区分坏重复、词库项或设计性保留。",
                ),
                "读 `template-backlog/SUMMARY.md` 和 `CANDIDATES.json`，决定写回模板库、词库、规则，还是标记为 keep。",
                "template_backlog",
            ),
        );
    }

    let mut ordered_assignments = assignments;
    ordered_assignments.sort_by_key(assignment_sort_key);
    let p1_items: Vec<&Assignment> = ordered_assignments
        .iter()
        .filter(|item| matches!(item.priority.as_str(), "P0" | "P1"))
        .collect();
    let p2_items: Vec<&Assignment> = ordered_assignments
        .iter()
        .filter(|item| item.priority == "P2")
        .collect();
    if !p2_items.is_empty() && p1_items.len() >= 12 {
        let mut out: Vec<Assignment> = p1_items.iter().take(12).map(|a| (*a).clone()).collect();
        out.extend(p2_items.iter().take(2).map(|a| (*a).clone()));
        Ok(out)
    } else {
        ordered_assignments.truncate(14);
        Ok(ordered_assignments)
    }
}

/// 故事级 review kit。
fn build_story_review_kit(
    story_dir: &Path,
    ordered: &[(PathBuf, &Analysis)],
    snapshots: &[(PathBuf, StoryConflictSnapshot)],
    labels: &EndingLabels,
    engine: &PlanEngine,
    output_root: Option<&Path>,
) -> Result<String> {
    let mut ordered_items: Vec<(PathBuf, &Analysis)> =
        ordered.iter().map(|(p, a)| (p.clone(), *a)).collect();
    ordered_items.sort_by_key(|(p, _)| chapter_sort_key(p));

    let layout = engine.paths();
    let scorecard_summary_path =
        scorecard::scorecard_path_for(&ordered_items[0].0, layout, output_root)?
            .parent()
            .context("scorecard 路径无父目录")?
            .join("SUMMARY.md");
    let learning_summary_path =
        learning::learning_log_path_for(&ordered_items[0].0, layout, output_root)?
            .parent()
            .context("learning 路径无父目录")?
            .join("SUMMARY.md");
    let profile_summary_path = profiles::profile_path_for(&ordered_items[0].0, None, layout)?
        .parent()
        .context("profile 路径无父目录")?
        .join("SUMMARY.md");
    let template_backlog_summary_path =
        backlog::backlog_path_for(&ordered_items[0].0, layout, output_root)?;
    let template_backlog_candidates_path =
        backlog::candidates_path_for(&ordered_items[0].0, layout, output_root)?;

    let mut gate_counter: Ctr = Ctr::default();
    let mut recommendation_counter: Ctr = Ctr::default();
    let mut pending_rows = 0;
    let mut consistency_story = String::new();
    let mut review_queue_command = String::new();
    let mut feedback_summary_command = String::new();
    let mut template_counter: Ctr = Ctr::default();

    for (draft_path, analysis) in &ordered_items {
        let snapshot = snapshots
            .iter()
            .find(|(p, _)| p == draft_path)
            .map(|(_, s)| s);
        if let Some(snap) = snapshot {
            if snap.available {
                pending_rows = pending_rows.max(snap.pending_rows.len());
                consistency_story = snap.story.clone().unwrap_or_default();
                if let Some(cmd) = &snap.review_queue_command {
                    review_queue_command = cmd.clone();
                }
                if let Some(cmd) = &snap.feedback_summary_command {
                    feedback_summary_command = cmd.clone();
                }
            }
        }
        let novel_dir = scorecard::novel_dir_for_draft(draft_path, engine.paths());
        let align = match &novel_dir {
            Some(dir) => {
                alignment::build_plan_draft_alignment(engine, draft_path, Some(dir), analysis)?
            }
            None => alignment::Alignment::novel_dir_not_resolved(None),
        };
        let axes = scorecard::build_axes(analysis, snapshot, Some(&align), None);
        let (gate, _priority, recommendation) = scorecard::decide_gate(analysis, &axes);
        gate_counter.add(&gate, 1);
        recommendation_counter.add(&recommendation, 1);
        for item in learning::collect_template_backlog(analysis) {
            template_counter.add(&format!("{}::{}", item.bucket, item.name), 1);
        }
    }

    let mut lines: Vec<String> = vec!["# Review Kit".to_string(), String::new()];
    lines.push(format!("- story: `{}`", story_dir.display()));
    lines.push(format!("- chapters: `{}`", ordered.len()));
    lines.push(format!(
        "- scorecards: `{}`",
        scorecard_summary_path.display()
    ));
    lines.push(format!("- learning: `{}`", learning_summary_path.display()));
    lines.push(format!("- profiles: `{}`", profile_summary_path.display()));
    lines.push(format!(
        "- template_backlog: `{}`",
        template_backlog_summary_path.display()
    ));
    lines.push(format!(
        "- template_candidates: `{}`",
        template_backlog_candidates_path.display()
    ));
    if !review_queue_command.is_empty() {
        lines.push(format!("- review_queue: `{review_queue_command}`"));
    }
    if !feedback_summary_command.is_empty() {
        lines.push(format!("- feedback_summary: `{feedback_summary_command}`"));
    }
    lines.push(String::new());

    lines.push("## Current State".to_string());
    for (name, count) in gate_counter.items() {
        lines.push(format!("- gate `{name}` x{count}"));
    }
    for (name, count) in recommendation_counter.items() {
        lines.push(format!("- recommendation `{name}` x{count}"));
    }
    lines.push(format!("- pending_consistency_rows: `{pending_rows}`"));
    if !consistency_story.is_empty() {
        lines.push(format!("- consistency_story: `{consistency_story}`"));
    }
    lines.push(String::new());

    let assignments =
        collect_review_assignments(story_dir, &ordered_items, snapshots, labels, engine)?;
    lines.push("## Review Assignments".to_string());
    if !assignments.is_empty() {
        for item in &assignments {
            lines.push(format!(
                "- {}: {}（source=`{}`）：{} 动作：{}",
                item.priority, item.title, item.source, item.reason, item.action
            ));
        }
    } else {
        lines.push(
            "- P2: 暂无硬派单；按 Suggested Flow 做常规抽查，重点确认可保留重复不要被误杀。"
                .to_string(),
        );
    }
    lines.push(String::new());

    lines.push("## Suggested Flow".to_string());
    lines.push("1. 先读 `Review Assignments`，按 P1/P2 处理可指派复审任务。".to_string());
    lines.push(
        "2. 再读 `scorecards/SUMMARY.md`，确认这条 Story 当前是 `PASS / WATCH / FAIL` 哪一侧。"
            .to_string(),
    );
    lines.push(
        "3. 再读 `learning/SUMMARY.md`，确认重复模板、沉淀目标和 plan-draft 漂移是否集中。"
            .to_string(),
    );
    lines.push(
        "4. 再读 `profiles/SUMMARY.md`，确认句式骨架、人物声音和场景色调是不是同一类问题反复出现。"
            .to_string(),
    );
    lines.push(
        "5. 再读 `template-backlog/SUMMARY.md`，把坏模式候选和可保留风格候选拆开看。".to_string(),
    );
    if !review_queue_command.is_empty() {
        lines.push(format!(
            "6. 跑 `{review_queue_command}`，逐条复核一致性 pending。"
        ));
    }
    if !feedback_summary_command.is_empty() {
        lines.push(format!(
            "7. 每做完一轮判定后，跑 `{feedback_summary_command}` 看这条 Story 是否开始收敛。"
        ));
    }
    lines.push(
        "8. 最后再决定这轮该沉淀模板、词库、规则，还是回修正文 / chapter-plan / story-plan。"
            .to_string(),
    );
    lines.push(String::new());

    lines.push("## Template Hotspots".to_string());
    if !template_counter.is_empty() {
        for (name, count) in template_counter.most_common(8) {
            lines.push(format!("- `{name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Chapter Paths".to_string());
    for (draft_path, _analysis) in &ordered_items {
        lines.push(format!("- `{}`", draft_path.display()));
    }
    lines.push(String::new());

    Ok(lines.join("\n") + "\n")
}

/// 逐章写三类单章报告 → 按 story 写 SUMMARY 与 review kit。
/// 返回（退出码, 应打印路径序列）。
pub fn run(opts: &KitOptions) -> Result<(i32, Vec<PathBuf>)> {
    let files = collect_chapter_files(&opts.paths)?;
    if files.is_empty() {
        eprintln!("No draft chapter files found.");
        return Ok((1, Vec::new()));
    }

    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let plan_engine = PlanEngine::new(&rules.plan, &rules.paths)?;
    let ctx = DraftContext::new(rules.clone())?;
    let template_bank = build_template_bank(ctx.draft_rules());
    let corpus_profile = build_corpus_profile(&ctx, &ctx.corpus_paths_for_targets(&files), &files)?;
    let labels = ctx.draft_rules().ending_labels.clone();

    let mut analyses: Vec<(PathBuf, Analysis)> = Vec::new();
    for path in &files {
        let analysis = analyze_path(
            &ctx,
            path,
            &template_bank,
            ctx.draft_rules().tracked_terms.as_slice(),
            corpus_profile.as_ref(),
            opts.sample_limit,
        )
        .with_context(|| format!("无法分析章节 {}", path.display()))?;
        analyses.push((path.clone(), analysis));
    }

    let mut snapshots: Vec<(PathBuf, StoryConflictSnapshot)> = Vec::new();
    for (path, _) in &analyses {
        let snapshot = build_story_conflict_snapshot_from_path(path, 200)?;
        snapshots.push((path.clone(), snapshot));
    }

    let mut printed: Vec<PathBuf> = Vec::new();
    for (draft_path, analysis) in &analyses {
        let snapshot = snapshots
            .iter()
            .find(|(p, _)| p == draft_path)
            .map(|(_, s)| s)
            .context("一致性快照缺失")?;
        write_text(
            &scorecard::scorecard_path_for(
                draft_path,
                plan_engine.paths(),
                opts.output_root.as_deref(),
            )?,
            &scorecard::build_scorecard_report(
                &plan_engine,
                &labels,
                draft_path,
                analysis,
                Some(snapshot),
                None,
            )?,
        )?;
        write_text(
            &learning::learning_log_path_for(
                draft_path,
                plan_engine.paths(),
                opts.output_root.as_deref(),
            )?,
            &learning::build_learning_log(&plan_engine, &labels, draft_path, analysis, snapshot)?,
        )?;
        write_text(
            &profiles::profile_path_for(draft_path, None, plan_engine.paths())?,
            &profiles::build_profile_report(draft_path, analysis, opts.sample_limit),
        )?;
    }

    // 按父目录分组（首现序），再按 story 目录字典序处理。
    let mut groups: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for (index, (path, _)) in analyses.iter().enumerate() {
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        if let Some(group) = groups.iter_mut().find(|g| g.0 == parent) {
            group.1.push(index);
        } else {
            groups.push((parent, vec![index]));
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    for (story_dir, indexes) in &groups {
        let items: Vec<(PathBuf, &Analysis)> = indexes
            .iter()
            .map(|&i| (analyses[i].0.clone(), &analyses[i].1))
            .collect();
        let snapshot_refs: Vec<(PathBuf, &StoryConflictSnapshot)> = indexes
            .iter()
            .map(|&i| (snapshots[i].0.clone(), &snapshots[i].1))
            .collect();

        let score_summary_path = scorecard::scorecard_path_for(
            &items[0].0,
            plan_engine.paths(),
            opts.output_root.as_deref(),
        )?
        .parent()
        .context("scorecard 路径无父目录")?
        .join("SUMMARY.md");
        write_text(
            &score_summary_path,
            &scorecard::build_story_summary(story_dir, &items, &snapshots, &labels, &plan_engine)?,
        )?;
        printed.push(score_summary_path);

        let learning_summary_path = learning::learning_log_path_for(
            &items[0].0,
            plan_engine.paths(),
            opts.output_root.as_deref(),
        )?
        .parent()
        .context("learning 路径无父目录")?
        .join("SUMMARY.md");
        write_text(
            &learning_summary_path,
            &learning::build_story_summary(
                &plan_engine,
                &labels,
                story_dir,
                &items,
                &snapshot_refs,
            )?,
        )?;
        printed.push(learning_summary_path);

        let profile_summary_path =
            profiles::profile_path_for(&items[0].0, None, plan_engine.paths())?
                .parent()
                .context("profile 路径无父目录")?
                .join("SUMMARY.md");
        write_text(
            &profile_summary_path,
            &profiles::build_story_summary(story_dir, &items, opts.sample_limit),
        )?;
        printed.push(profile_summary_path);

        let (template_backlog_markdown, template_backlog_payload) =
            backlog::build_story_backlog(story_dir, &items)?;
        let template_backlog_summary_path = backlog::backlog_path_for(
            &items[0].0,
            plan_engine.paths(),
            opts.output_root.as_deref(),
        )?;
        let template_backlog_candidates_path = backlog::candidates_path_for(
            &items[0].0,
            plan_engine.paths(),
            opts.output_root.as_deref(),
        )?;
        write_text(&template_backlog_summary_path, &template_backlog_markdown)?;
        write_json(&template_backlog_candidates_path, &template_backlog_payload)?;
        printed.push(template_backlog_summary_path);
        printed.push(template_backlog_candidates_path);

        let kit_path = review_kit_path_for(
            &items[0].0,
            plan_engine.paths(),
            opts.output_root.as_deref(),
        )?;
        write_text(
            &kit_path,
            &build_story_review_kit(
                story_dir,
                &items,
                &snapshots,
                &labels,
                &plan_engine,
                opts.output_root.as_deref(),
            )?,
        )?;
        printed.push(kit_path);
    }

    Ok((0, printed))
}
