//! `reports-learning` 子命令：
//! 草稿章评审学习日志（learning/*.md + 每 story 目录 SUMMARY.md 镜像树）。
//!
//! - 章节分析复用 `audit::draft::analyze_path`（语料学习缺省开启）；
//! - 一致性快照来自 `consistency::build_story_conflict_snapshot`；
//! - 对齐信号来自 `reports::alignment`；
//! - 章末标签/连续段复用 `stats::draft`（`infer_ending_label`/`summarize_runs`/
//!   `ending_flow_text`，展示名走 `EndingLabels` 查表）；
//! - 中文文案固定；排序/截断语义稳定（stable 序 + 首现序）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::draft::{analyze_path, build_corpus_profile, Analysis, DraftContext};
use crate::audit::plan::PlanEngine;
use crate::config::{self, EndingLabels};
use crate::consistency::{build_story_conflict_snapshot, BacklogItem, StoryConflictSnapshot};
use crate::input::write_text;
use crate::reports::alignment::{build_plan_draft_alignment, Alignment};
use crate::reports::scorecard::novel_dir_for_draft;
use crate::rules::build_template_bank;
use crate::stats::draft::{
    chapter_sort_key, collect_chapter_files, ending_flow_text, infer_ending_label, stats_path_for,
    summarize_runs,
};
use crate::stats::Ctr;

/// `reports-learning` 子命令参数（位置 `paths` nargs+、
/// `--sample-limit` 缺省 6）。
#[derive(Debug, Clone)]
pub struct LearningOptions {
    /// 产物根目录；缺省写在输入树旁。
    pub output_root: Option<PathBuf>,
    /// 位置参数：草稿章文件或目录。
    pub paths: Vec<PathBuf>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// 每条规则最多记录的样本行数（默认 6）。
    pub sample_limit: usize,
}

/// 学习清单条目（`{bucket, name, reason, sample}`；bonus 条目 bucket 为空串）。
#[derive(Debug, Clone, Default)]
pub struct BacklogEntry {
    /// 模板桶名（`patterns`/`custom_template`/...；bonus 条目无桶）。
    pub bucket: String,
    /// 候选名。
    pub name: String,
    /// 建议原因（中文文案逐字照抄）。
    pub reason: String,
    /// 样例文本（可为空串）。
    pub sample: String,
}

/// 规则/沉淀建议（`{target, reason}`）。
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// 沉淀目标路径（如 `configs/rules/review.yaml#draft.template_rules`）。
    pub target: String,
    /// 建议原因（中文文案逐字照抄）。
    pub reason: String,
}

/// 取字符串前 n 个码点。
fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// bool 渲染（`True`/`False`）。
fn bool_str(flag: bool) -> &'static str {
    if flag {
        "True"
    } else {
        "False"
    }
}

/// 章末标签展示名（对齐 `stats.draft.ending_label_display` 的查表语义）。
fn ending_display(labels: &EndingLabels, label: &str) -> String {
    labels
        .display
        .get(label)
        .cloned()
        .unwrap_or_else(|| label.to_string())
}

/// `learning_log_path_for`：`stats_path_for(draft).parent / learning / {stem}.md`。
pub fn learning_log_path_for(
    draft_path: &Path,
    paths: &crate::config::PathsConfig,
    output_root: Option<&Path>,
) -> Result<PathBuf> {
    let stats = stats_path_for(draft_path, output_root, paths)?;
    let stem = draft_path
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("章节文件名必须为合法 UTF-8: {}", draft_path.display()))?;
    Ok(stats
        .parent()
        .context("stats 路径无父目录")?
        .join("learning")
        .join(format!("{stem}.md")))
}

/// `bucket_priority`：桶优先级（未知名 → 99）。
fn bucket_priority(bucket: &str) -> i32 {
    match bucket {
        "patterns" => 0,
        "custom_template" => 1,
        "phrases" => 2,
        "tokens" => 3,
        "punctuation" => 4,
        "sentence_length" => 5,
        "fatigue_window" => 6,
        "ba_operation_context" => 7,
        "tracked_term" => 8,
        "learned_filter" => 9,
        _ => 99,
    }
}

/// `collect_template_backlog`：`template_candidates` 前 20 条按
/// 名字分组 → 组内按（优先级, 桶名）取首条 → 组间按最小优先级 stable 排 → 截 12。
pub fn collect_template_backlog(analysis: &Analysis) -> Vec<BacklogEntry> {
    let mut grouped: Vec<(String, Vec<BacklogEntry>)> = Vec::new();
    for item in analysis.template_candidates.iter().take(20) {
        let entry = BacklogEntry {
            bucket: item.candidate_type.clone(),
            name: item.name.clone(),
            reason: item.note.clone(),
            sample: item.sample.trim().to_string(),
        };
        if let Some(group) = grouped.iter_mut().find(|g| g.0 == entry.name) {
            group.1.push(entry);
        } else {
            grouped.push((entry.name.clone(), vec![entry]));
        }
    }
    grouped.sort_by_key(|group| {
        group
            .1
            .iter()
            .map(|e| bucket_priority(&e.bucket))
            .min()
            .unwrap_or(99)
    });
    let mut backlog: Vec<BacklogEntry> = Vec::new();
    for (_, mut items) in grouped {
        items.sort_by(|a, b| {
            (bucket_priority(&a.bucket), &a.bucket).cmp(&(bucket_priority(&b.bucket), &b.bucket))
        });
        backlog.push(items.remove(0));
    }
    backlog.truncate(12);
    backlog
}

/// `collect_bonus_backlog`：6 类加分候选（截 6）。
pub fn collect_bonus_backlog(analysis: &Analysis) -> Vec<BacklogEntry> {
    let mut backlog: Vec<BacklogEntry> = Vec::new();
    let entry = |name: &str, reason: &str, sample: String| BacklogEntry {
        bucket: String::new(),
        name: name.to_string(),
        reason: reason.to_string(),
        sample,
    };

    let ending = &analysis.ending;
    if !ending.warn && !ending.tail_excerpt.is_empty() {
        backlog.push(entry(
            "章末收束",
            "本章章末没有命中模板警告，可人工确认它是不是值得保留的收束方式。",
            prefix_chars(&ending.tail_excerpt, 80),
        ));
    }
    let aa_bb = &analysis.aa_bb_patterns;
    if !aa_bb.is_empty() && !aa_bb.iter().any(|p| p.warn) {
        let first = &aa_bb[0];
        backlog.push(entry(
            "轻量排比或重叠词",
            "检测到少量节奏性排比，但没有达到疲劳阈值，可人工判断是不是文气亮点。",
            first.samples.first().cloned().unwrap_or_default(),
        ));
    }
    if analysis.dialogue.dialogue_axis_gaps.is_empty()
        && (0.12..=0.45).contains(&analysis.summary.quote_ratio)
    {
        backlog.push(entry(
            "对白调度",
            "对白比例和转轴暂时正常，可人工确认角色声音是否成立。",
            String::new(),
        ));
    }
    let de = &analysis.dialogue_emotions;
    if de.dialogue_sentences >= 4 && !de.flatness_warn && !de.volatility_warn && de.shift_count >= 1
    {
        backlog.push(entry(
            "对白情绪曲线",
            "对白情绪出现了可解释起伏，可人工确认它是不是人物关系推进而不是工具误判。",
            String::new(),
        ));
    }
    let cv = &analysis.character_voice;
    if cv.speaker_count >= 2 && !cv.warn && cv.coverage_ratio >= 0.35 {
        backlog.push(entry(
            "角色声音分化",
            "本章已有可识别说话人分布且暂未出现明显同腔，可人工确认人物声音是否真的拉开了差异。",
            String::new(),
        ));
    }
    let bp = &analysis.battle_profile;
    if bp.sequence_count >= 1 && bp.result_ratio >= 0.35 {
        backlog.push(entry(
            "动作结果链",
            "冲突段不仅有动作，还有结果或伤害反馈，可人工确认是否值得沉淀成战斗模板样本。",
            String::new(),
        ));
    }
    backlog.truncate(6);
    backlog
}

/// `collect_rule_suggestions`：风格/规则侧建议（截 6）。
pub fn collect_rule_suggestions(analysis: &Analysis) -> Vec<Suggestion> {
    let suggestion = |target: &str, reason: &str| Suggestion {
        target: target.to_string(),
        reason: reason.to_string(),
    };
    let mut suggestions: Vec<Suggestion> = Vec::new();
    if analysis.sentence_patterns.len() >= 4 {
        suggestions.push(suggestion(
            "configs/rules/review.yaml#draft.template_rules",
            "句首骨架重复已经形成明确家族，可考虑把高频骨架固化成模板库条目。",
        ));
    }
    if analysis.tracked_term_window_count >= 3 {
        suggestions.push(suggestion(
            "configs/rules/review.yaml#draft.tracked_terms",
            "局部点名密度偏高，说明某些实体或动作词值得进入跟踪词库。",
        ));
    }
    if analysis.viewpoint_profile.warn {
        suggestions.push(suggestion(
            "skills/review-guide.md",
            "视角锚点漂移已经能被脚本稳定抓到，评审指南应补“近距离视角切锚”的具体复核动作。",
        ));
    }
    if analysis.battle_profile.warn || analysis.scene_map.warn {
        suggestions.push(suggestion(
            "configs/rules/review.yaml#draft.template_rules",
            "场面功能失衡或动作链缺结果已开始出现，可继续补充对应模板与反模板样本。",
        ));
    }
    if analysis
        .review_reminders
        .iter()
        .any(|r| r.priority == "P1" && r.category == "dialogue")
    {
        suggestions.push(suggestion(
            "skills/review-guide.md",
            "对白问题已经稳定到 P1，说明评审指南里应进一步补具体审查动作。",
        ));
    }
    if analysis.character_voice.warn {
        suggestions.push(suggestion(
            "skills/review-guide.md",
            "角色对白同质化已经能被脚本稳定提示，评审指南应补“角色声音拉差”的复核动作。",
        ));
    }
    if analysis.summary.warn_sections >= 14 {
        suggestions.push(suggestion(
            "novel1/rules/draft.md",
            "当前章的问题组合已经足够重，若在同一 Story 反复出现，应沉淀为本书规则。",
        ));
    }
    suggestions.truncate(6);
    suggestions
}

/// `build_consistency_suggestions`：一致性快照侧建议（`None`/不可用 → 空；截 6）。
pub fn build_consistency_suggestions(snapshot: Option<&StoryConflictSnapshot>) -> Vec<Suggestion> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if !snapshot.available {
        return Vec::new();
    }
    let suggestion = |target: &str, reason: String| Suggestion {
        target: target.to_string(),
        reason,
    };
    let mut suggestions: Vec<Suggestion> = Vec::new();

    if snapshot.decision_counter.get("false_positive") >= 2 {
        suggestions.push(suggestion(
            "consistency",
            "同一 story 已累计多条一致性误报，说明抽取逻辑该继续压噪，而不是把人工复核当常态。"
                .into(),
        ));
    }
    if snapshot.decision_counter.get("confirmed") >= 2 {
        suggestions.push(suggestion(
            "novel1/rules/draft.md",
            "同一 story 已确认多条一致性问题，说明这类漂移不是偶发手误，值得沉淀为本书返工规则。"
                .into(),
        ));
    }
    if snapshot
        .category_counter
        .items()
        .iter()
        .any(|(name, _)| name.ends_with("::designed_keep"))
    {
        suggestions.push(suggestion(
            "configs/rules/review.yaml#draft.template_rules",
            "已有一致性候选被人工判为设计性保留，说明某些重复或称谓变化应进入可保留模式样本，而不是继续当纯风险。".into(),
        ));
    }
    if !snapshot.pending_rows.is_empty() {
        suggestions.push(suggestion(
            "novel1/research/consistency/review-feedback.jsonl",
            format!(
                "当前 story 还有 {} 条中高置信度一致性候选未复核，先补反馈再决定是否继续扩规则。",
                snapshot.pending_rows.len()
            ),
        ));
    }
    suggestions.extend(
        snapshot
            .global_feedback_backlog
            .iter()
            .take(2)
            .map(|item| Suggestion {
                target: item.target.clone(),
                reason: item.reason.clone(),
            }),
    );
    suggestions.truncate(6);
    suggestions
}

/// `build_learning_log`：单章学习日志 markdown（逐字渲染）。
///
/// 对齐需 `PlanEngine`（施工图信号）与 `EndingLabels`（章末展示名）参数；
/// `snapshot` 恒有（不可用时渲染「无」分支）。
pub fn build_learning_log(
    engine: &PlanEngine,
    labels: &EndingLabels,
    draft_path: &Path,
    analysis: &Analysis,
    snapshot: &StoryConflictSnapshot,
) -> Result<String> {
    let novel_dir = novel_dir_for_draft(draft_path, engine.paths());
    let alignment = match &novel_dir {
        Some(dir) => build_plan_draft_alignment(engine, draft_path, Some(dir), analysis)?,
        None => Alignment::novel_dir_not_resolved(None),
    };
    let template_backlog = collect_template_backlog(analysis);
    let bonus_backlog = collect_bonus_backlog(analysis);
    let mut rule_suggestions = collect_rule_suggestions(analysis)
        .into_iter()
        .chain(build_consistency_suggestions(Some(snapshot)))
        .collect::<Vec<_>>();
    if alignment.available && alignment.mismatch_count.unwrap_or(0) >= 1 {
        rule_suggestions.push(Suggestion {
            target: alignment
                .chapter_plan_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            reason: "正文功能与施工图存在漂移，这一轮复核不要只改句子，先确认 `本章功能 / 章节收尾` 是否需要回修。".into(),
        });
    }

    let stem = draft_path
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("章节文件名必须为合法 UTF-8: {}", draft_path.display()))?;
    let mut lines: Vec<String> = vec![format!("# {stem} Review Learning Log"), String::new()];
    lines.push(format!("- source: `{}`", draft_path.display()));
    lines.push(
        "- usage: 这不是最终审查结论，而是为‘确认 / 驳回 / 沉淀’准备的学习清单。".to_string(),
    );
    lines.push(String::new());

    lines.push("## Manual Decisions".to_string());
    lines.push("- confirmed_issues: `_fill_in_`".to_string());
    lines.push("- false_positives: `_fill_in_`".to_string());
    lines.push("- design_repeats_to_keep: `_fill_in_`".to_string());
    lines.push("- missing_checks: `_fill_in_`".to_string());
    lines.push(String::new());

    lines.push("## Template Backlog".to_string());
    if !template_backlog.is_empty() {
        for item in &template_backlog {
            lines.push(format!(
                "- `[pending]` `{}` `{}`：{}",
                item.bucket, item.name, item.reason
            ));
            if !item.sample.is_empty() {
                lines.push(format!("  样例：{}", item.sample));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Bonus Candidates".to_string());
    if !bonus_backlog.is_empty() {
        for item in &bonus_backlog {
            lines.push(format!("- `[pending]` `{}`：{}", item.name, item.reason));
            if !item.sample.is_empty() {
                lines.push(format!("  样例：{}", item.sample));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Rule / Bank Suggestions".to_string());
    if !rule_suggestions.is_empty() {
        for item in &rule_suggestions {
            lines.push(format!("- `[pending]` `{}`：{}", item.target, item.reason));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Consistency Feedback Snapshot".to_string());
    if snapshot.available {
        lines.push(format!(
            "- story: `{}`",
            snapshot.story.clone().unwrap_or_default()
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
        lines.push(format!("- pending=`{}`", snapshot.pending_rows.len()));
        lines.push(format!(
            "- review_queue: `{}`",
            snapshot.review_queue_command.clone().unwrap_or_default()
        ));
        lines.push(format!(
            "- feedback_summary: `{}`",
            snapshot
                .feedback_summary_command
                .clone()
                .unwrap_or_default()
        ));
        if !snapshot.facet_counter.is_empty() {
            lines.push("- facets:".to_string());
            for (name, count) in snapshot.facet_counter.most_common(6) {
                lines.push(format!("  - `{name}` x{count}"));
            }
        }
        let pending_rows: Vec<&crate::consistency::ConflictRow> =
            snapshot.pending_rows.iter().take(4).collect();
        if !pending_rows.is_empty() {
            lines.push("- pending rows:".to_string());
            for row in pending_rows {
                lines.push(format!(
                    "  - `{}` `{}` confidence=`{}` {}",
                    row.category, row.title, row.confidence, row.summary
                ));
            }
        }
        if !snapshot.pending_actions.is_empty() {
            lines.push("- pending actions:".to_string());
            for item in snapshot.pending_actions.iter().take(3) {
                lines.push(format!(
                    "  - `{}` `{}` confidence=`{}`：{}",
                    item.category, item.title, item.confidence, item.focus
                ));
                lines.push(format!("  - command: `{}`", item.command));
            }
        }
    } else {
        lines.push("- 无一致性反馈快照；可能还没建立本地索引库。".to_string());
    }
    lines.push(String::new());

    lines.push("## Plan Alignment Review".to_string());
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
            alignment.plan_chapter_function.clone().unwrap_or_default(),
            alignment.draft_chapter_function.clone().unwrap_or_default(),
            bool_str(alignment.chapter_match.unwrap_or(false))
        ));
        lines.push(format!(
            "- ending_function: plan=`{}` draft=`{}` match=`{}`",
            alignment.plan_ending_function.clone().unwrap_or_default(),
            alignment.draft_ending_function.clone().unwrap_or_default(),
            bool_str(alignment.ending_match.unwrap_or(false))
        ));
        lines.push(format!(
            "- alignment_status=`{}`",
            alignment
                .alignment_status
                .clone()
                .unwrap_or_else(|| "unknown".into())
        ));
        lines.push(format!(
            "- recommended_action=`{}`",
            alignment
                .recommended_action
                .clone()
                .unwrap_or_else(|| "manual_review".into())
        ));
        if alignment
            .drift_types
            .as_deref()
            .is_some_and(|d| !d.is_empty())
        {
            lines.push(
                "- drift_types: ".to_string()
                    + &alignment
                        .drift_types
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
            );
        }
        if alignment.mismatch_count.unwrap_or(0) >= 1 {
            lines.push("- review_focus:".to_string());
            lines.push(format!(
                "  - {}",
                alignment
                    .review_note
                    .clone()
                    .unwrap_or_else(|| "先判断该修正文，还是回修 chapter-plan。".into())
            ));
            lines.push(
                "  - 如果正文更好，应回改施工图；如果施工图更对，应压回正文结构，而不是只做字词去重。".to_string(),
            );
        }
    } else {
        lines.push("- 无 plan-draft 对齐快照".to_string());
    }
    lines.push(String::new());

    lines.push("## Ending Signal Review".to_string());
    let ending_signal = infer_ending_label(analysis, labels);
    lines.push(format!(
        "- ending_signal=`{}`",
        ending_display(labels, &ending_signal)
    ));
    if analysis.ending.warn {
        lines.push(
            "- 当前章末仍有模板化风险；复盘时先判断它是坏重复，还是真有新的后果落点。".to_string(),
        );
    } else {
        lines.push(
            "- 当前章末未触发模板化警告；复盘时优先判断它是否在承担新的后果，而不是只因为不重复就直接加分。".to_string(),
        );
    }
    lines.push(String::new());

    lines.push("## Feedback-Derived Backlog".to_string());
    let global_backlog: Vec<&BacklogItem> = snapshot.global_feedback_backlog.iter().collect();
    if !global_backlog.is_empty() {
        for item in global_backlog.iter().take(6) {
            lines.push(format!("- `{}`：{}", item.target, item.reason));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Reminder Snapshot".to_string());
    if !analysis.review_reminders.is_empty() {
        for item in analysis.review_reminders.iter().take(8) {
            lines.push(format!(
                "- `{}` `{}` {}：{}",
                item.priority, item.category, item.title, item.reason
            ));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Next Review Questions".to_string());
    lines.push("- 这些重复里，哪些其实承担了人物声音、压迫感、节奏或讽刺功能？".to_string());
    lines.push("- 这章真正要沉淀的是模板、词库、规则，还是只是一个局部问题？".to_string());
    lines.push("- 如果这类问题再次出现，下一轮脚本应该如何更早抓到它？".to_string());
    lines.push(String::new());

    Ok(lines.join("\n") + "\n")
}

/// `build_story_summary`：story 级 SUMMARY.md（逐字渲染；`analyses` 内部按
/// `chapter_sort_key` 排序，`snapshots` 按章节路径查表，缺失视为不可用快照）。
pub fn build_story_summary(
    engine: &PlanEngine,
    labels: &EndingLabels,
    story_dir: &Path,
    analyses: &[(PathBuf, &Analysis)],
    snapshots: &[(PathBuf, &StoryConflictSnapshot)],
) -> Result<String> {
    let mut ordered: Vec<(PathBuf, &Analysis)> = analyses.to_vec();
    ordered.sort_by_key(|a| chapter_sort_key(&a.0));

    let mut template_counter = Ctr::default();
    let mut suggestion_counter = Ctr::default();
    let mut consistency_decisions = Ctr::default();
    let mut consistency_categories = Ctr::default();
    let mut consistency_facets = Ctr::default();
    let mut alignment_counter = Ctr::default();
    let mut ending_alignment_counter = Ctr::default();
    let mut ending_signal_counter = Ctr::default();
    let mut ending_signal_flow: Vec<String> = Vec::new();

    let mut lines: Vec<String> = vec!["# Review Learning Summary".to_string(), String::new()];
    lines.push(format!("- story: `{}`", story_dir.display()));
    lines.push(format!("- chapters: `{}`", ordered.len()));
    lines.push(String::new());

    lines.push("## Chapters".to_string());
    for (path, analysis) in &ordered {
        let template_backlog = collect_template_backlog(analysis);
        let snapshot = snapshots.iter().find(|(p, _)| p == path).map(|(_, s)| *s);
        let novel_dir = novel_dir_for_draft(path, engine.paths());
        let alignment = match &novel_dir {
            Some(dir) => build_plan_draft_alignment(engine, path, Some(dir), analysis)?,
            None => Alignment::unavailable(),
        };
        for item in &template_backlog {
            template_counter.add(&format!("{}::{}", item.bucket, item.name), 1);
        }
        for item in collect_rule_suggestions(analysis)
            .into_iter()
            .chain(build_consistency_suggestions(snapshot))
        {
            suggestion_counter.add(&item.target, 1);
        }
        if alignment.available {
            alignment_counter.add(
                &format!(
                    "{}->{}",
                    alignment.plan_chapter_function.clone().unwrap_or_default(),
                    alignment.draft_chapter_function.clone().unwrap_or_default()
                ),
                1,
            );
            ending_alignment_counter.add(
                &format!(
                    "{}->{}",
                    alignment.plan_ending_function.clone().unwrap_or_default(),
                    alignment.draft_ending_function.clone().unwrap_or_default()
                ),
                1,
            );
        }
        let ending_signal = infer_ending_label(analysis, labels);
        ending_signal_counter.add(&ending_signal, 1);
        ending_signal_flow.push(ending_signal.clone());
        if let Some(snap) = snapshot {
            if snap.available {
                for (decision, count) in snap.decision_counter.items() {
                    consistency_decisions.add(&decision, count);
                }
                for (category, count) in snap.category_counter.items() {
                    consistency_categories.add(&category, count);
                }
                for (facet, count) in snap.facet_counter.items() {
                    consistency_facets.add(&facet, count);
                }
            }
        }
        lines.push(format!(
            "- `{}` template_backlog=`{}` reminders=`{}` warn_sections=`{}` ending_signal=`{}`",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            template_backlog.len(),
            analysis.review_reminders.len(),
            analysis.summary.warn_sections,
            ending_display(labels, &ending_signal)
        ));
    }
    lines.push(String::new());

    lines.push("## Repeated Template Candidates".to_string());
    if !template_counter.is_empty() {
        for (name, count) in template_counter.most_common(12) {
            lines.push(format!("- `{name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
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
        lines.push(format!(
            "- repeated=`{}`",
            if runs.is_empty() {
                "无".to_string()
            } else {
                runs.join(" | ")
            }
        ));
        if !runs.is_empty() {
            lines.push(
                "- review_focus: 连续同类章末时，优先判断这些结尾是在推进不同后果，还是只是在复用同一种收束手势。".to_string(),
            );
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Suggested Deposition Targets".to_string());
    if !suggestion_counter.is_empty() {
        for (name, count) in suggestion_counter.most_common_all() {
            lines.push(format!("- `{name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Consistency Feedback".to_string());
    if !consistency_decisions.is_empty() {
        for decision in ["confirmed", "false_positive", "designed_keep", "watch"] {
            lines.push(format!(
                "- `{decision}` x{}",
                consistency_decisions.get(decision)
            ));
        }
        if !consistency_categories.is_empty() {
            lines.push("- categories:".to_string());
            for (name, count) in consistency_categories.most_common(8) {
                lines.push(format!("  - `{name}` x{count}"));
            }
        }
        if !consistency_facets.is_empty() {
            lines.push("- facets:".to_string());
            for (name, count) in consistency_facets.most_common(8) {
                lines.push(format!("  - `{name}` x{count}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    Ok(lines.join("\n") + "\n")
}

/// 收集章节 → 分析 → 一致性快照 → 写 `learning/*.md`
/// （先全部章节）→ 按 story 写 `SUMMARY.md`。返回（退出码, 应打印路径序列）。
pub fn run(opts: &LearningOptions) -> Result<(i32, Vec<PathBuf>)> {
    let files = collect_chapter_files(&opts.paths)?;
    if files.is_empty() {
        eprintln!("No draft chapter files found.");
        return Ok((1, Vec::new()));
    }

    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let plan_engine = PlanEngine::new(&rules.plan, &rules.paths)?;
    let ctx = DraftContext::new(rules)?;
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
        snapshots.push((path.clone(), build_story_conflict_snapshot(path)?));
    }

    let mut printed: Vec<PathBuf> = Vec::new();
    for (path, analysis) in &analyses {
        let snapshot = &snapshots
            .iter()
            .find(|(p, _)| p == path)
            .expect("每章快照已生成")
            .1;
        let out_path =
            learning_log_path_for(path, plan_engine.paths(), opts.output_root.as_deref())?;
        write_text(
            &out_path,
            &build_learning_log(&plan_engine, &labels, path, analysis, snapshot)?,
        )?;
        printed.push(out_path);
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
        let snap_refs: Vec<(PathBuf, &StoryConflictSnapshot)> =
            snapshots.iter().map(|(p, s)| (p.clone(), s)).collect();
        let summary_path = learning_log_path_for(
            &items[0].0,
            plan_engine.paths(),
            opts.output_root.as_deref(),
        )?
        .parent()
        .context("learning 路径无父目录")?
        .join("SUMMARY.md");
        let summary = build_story_summary(&plan_engine, &labels, story_dir, &items, &snap_refs)?;
        write_text(&summary_path, &summary)?;
        printed.push(summary_path);
    }
    Ok((0, printed))
}
