//! `reports-scorecard` 子命令：
//! 草稿章评审记分卡（scorecards/*.md + 每 story 目录 SUMMARY.md 镜像树）。
//!
//! - 章节分析复用 `audit::draft::analyze_path`（语料学习缺省开启）；
//! - 一致性快照来自 `consistency::build_story_conflict_snapshot`；
//! - 对齐信号来自 `reports::alignment`；
//! - 中文文案固定；浮点输出走 `audit::draft::float_repr`（浮点展示语义）。
//!
//! 子模块：`axes`（评审轴/加分候选/门禁判定）、`trend`（跨章合流快照）、
//! `report`（单章记分卡与 story 级 SUMMARY.md 渲染）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::draft::{
    analyze_path, build_corpus_profile, float_repr, Analysis, Counter, DraftContext, HardFlag,
    ReviewReminder,
};
use crate::audit::plan::PlanEngine;
use crate::config::{self, EndingLabels};
use crate::consistency::{build_story_conflict_snapshot, BacklogItem, StoryConflictSnapshot};
use crate::input::write_text;
use crate::reports::alignment;
use crate::rules::{build_template_bank, round2};
use crate::stats::draft::{
    chapter_sort_key, collect_chapter_files, ending_flow_text, infer_ending_label, stats_path_for,
    summarize_runs,
};

/// `reports-scorecard` 子命令参数（位置 `paths` nargs+、
/// `--sample-limit` 缺省 6）。
#[derive(Debug, Clone)]
pub struct ScorecardOptions {
    /// 产物根目录；缺省写在输入树旁。
    pub output_root: Option<PathBuf>,
    /// 位置参数：草稿章文件或目录。
    pub paths: Vec<PathBuf>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// 每条规则最多记录的样本行数（默认 6）。
    pub sample_limit: usize,
}

/// `lib.paths.novel_dir_for_draft`：最近的正文目录祖先时返回其父目录。
#[must_use]
pub fn novel_dir_for_draft(
    draft_path: &Path,
    paths: &crate::config::PathsConfig,
) -> Option<PathBuf> {
    for parent in draft_path.ancestors().skip(1) {
        if paths.drafts_root_component(parent).is_some() {
            return parent
                .parent()
                .map(Path::to_path_buf)
                .or_else(|| Some(parent.to_path_buf()));
        }
    }
    None
}

/// `scorecard_path_for`：`stats_path_for(draft).parent / scorecards / {stem}.md`。
pub fn scorecard_path_for(
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
        .join("scorecards")
        .join(format!("{stem}.md")))
}

/// 收集章节 → 分析 → 一致性快照 → 按 story 写
/// `scorecards/*.md` + `SUMMARY.md`。返回（退出码, 应打印路径序列）。
pub fn run(opts: &ScorecardOptions) -> Result<(i32, Vec<PathBuf>)> {
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
        let snapshot = build_story_conflict_snapshot(path)?;
        snapshots.push((path.clone(), snapshot));
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

    let mut printed: Vec<PathBuf> = Vec::new();
    for (story_dir, indexes) in &groups {
        let items: Vec<(PathBuf, &Analysis)> = indexes
            .iter()
            .map(|&i| (analyses[i].0.clone(), &analyses[i].1))
            .collect();
        let story_trend_snapshots = build_story_trend_snapshots(&items, &labels);
        for (draft_path, analysis) in &items {
            let out_path =
                scorecard_path_for(draft_path, ctx.paths(), opts.output_root.as_deref())?;
            let report = build_scorecard_report(
                &plan_engine,
                &labels,
                draft_path,
                analysis,
                snapshots
                    .iter()
                    .find(|(p, _)| p == draft_path)
                    .map(|(_, s)| s),
                story_trend_snapshots
                    .iter()
                    .find(|(p, _)| p == draft_path)
                    .map(|(_, t)| t),
            )?;
            write_text(&out_path, &report)?;
            printed.push(out_path);
        }
        let summary_path =
            scorecard_path_for(&items[0].0, ctx.paths(), opts.output_root.as_deref())?
                .parent()
                .context("scorecard 路径无父目录")?
                .join("SUMMARY.md");
        let summary = build_story_summary(story_dir, &items, &snapshots, &labels, &plan_engine)?;
        write_text(&summary_path, &summary)?;
        printed.push(summary_path);
    }
    Ok((0, printed))
}

mod axes;
mod report;
mod trend;

pub use axes::{build_axes, build_bonus_candidates, decide_gate, Axis, BonusCandidate};
pub use report::{build_scorecard_report, build_story_summary};
pub use trend::{build_story_trend_snapshots, TrendSnapshot};
