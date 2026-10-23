//! `reports.workspace`：concept / plan / draft / consistency 四节 → 单份
//! AUDIT.md 工作区看板。
//!
//! - 各节复用既有模块（stats、audit、reports、consistency）的既有逻辑；
//!   本文件只做编排与看板渲染，渲染输出逐字节稳定。
//! - draft 节的浮点均值走 `rules::round2`（banker's）+ `audit::draft::float_repr`
//!   （浮点展示语义），与 scorecard 的 avg 渲染一致。
//!
//! 子模块：`tools`（共享小工具）、`collect`（concept/plan/consistency 节收集）、
//! `draft`（draft 节收集与 TEMPLATE_RESEARCH.md 生成）、
//! `trajectories`（轨迹/派生行 builder）、`dashboard`（AUDIT.md 看板渲染）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use fancy_regex::Regex;
use serde_json::Value;

use crate::audit::draft::{float_repr, Analysis, DraftContext};
use crate::audit::plan as plan_audit;
use crate::audit::plan::PlanEngine;
use crate::config::{self, EndingLabels};
use crate::consistency::{
    self, AlignmentRow, ConflictRow, GoalTensionRow, PairRow, RelationshipTensionRow, TensionRow,
};
use crate::input;
use crate::reports::alignment;
use crate::reports::backlog;
use crate::reports::catalog;
use crate::reports::kit;
use crate::reports::scorecard;
use crate::rules::{build_template_bank, round2};
use crate::stats::draft::{
    collect_chapter_files, ending_display, ending_flow_text, infer_ending_label, summarize_runs,
    AnalysisEnv,
};
use crate::stats::{concept as concept_stats, plan as plan_stats, Ctr};
use rusqlite::Connection;

/// 计划目录名。
/// 漂移样本上限。
const DRIFT_SAMPLE_LIMIT: usize = 6;

/// 章节 ID 正则（忽略大小写的 `ch<数字>`）。
static CHAPTER_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)ch(\\d+)").expect("CHAPTER_ID_RE 应可编译"));

/// `reports-workspace` 子命令参数。
#[derive(Debug, Clone)]
pub struct WorkspaceOptions {
    /// novel 目录（例如 novel1）。
    pub novel_dir: PathBuf,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// 每条规则最多记录的样本行数（缺省 3）。
    pub sample_limit: usize,
    /// 滚动章节窗口大小（缺省 [2, 3]；显式 `--window-sizes` 不带值时为空列表）。
    pub window_sizes: Vec<usize>,
    /// 产物根目录；缺省写在小说本地。
    pub output_root: Option<PathBuf>,
}

/// 四节收集 + 看板渲染 + 写 AUDIT.md，stdout 仅打印 AUDIT.md 路径。
pub fn run(opts: &WorkspaceOptions) -> Result<(i32, Vec<PathBuf>)> {
    let novel_dir = &opts.novel_dir;
    if !novel_dir.exists() {
        eprintln!("Novel directory not found: {}", novel_dir.display());
        return Ok((1, Vec::new()));
    }
    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let engine = PlanEngine::new(&rules.plan, &rules.paths)?;
    let layout = rules.paths.clone();
    let ctx = DraftContext::new(rules)?;
    let concept = collect_concept_section(novel_dir)?;
    let plans = collect_plan_section(novel_dir, &engine)?;
    let drafts = collect_draft_section(
        novel_dir,
        opts.sample_limit,
        &opts.window_sizes,
        &ctx,
        &engine,
        opts.output_root.as_deref(),
    )?;
    let consistency_section = collect_consistency_section(novel_dir, &layout)?;
    let dashboard = build_dashboard(novel_dir, &concept, &plans, &drafts, &consistency_section);
    let out_path = novel_dir.join("AUDIT.md");
    input::write_text(&out_path, &dashboard)?;
    Ok((0, vec![out_path]))
}

/// story 轨迹摘要行（`build_story_trajectory_summary` 输出）。
#[derive(Debug, Clone)]
pub struct TrajectorySummaryRow {
    pub story: String,
    pub state_summary: String,
    pub goal_summary: String,
    pub relationship_summary: String,
    pub samples: Vec<String>,
}

/// story 轨迹明细条目（`build_story_trajectory_details` 输出）。
#[derive(Debug, Clone)]
pub struct TrajectoryDetailItem {
    pub title: String,
    pub kind: &'static str,
    pub summary: String,
    pub timeline: Vec<String>,
}

/// story 轨迹明细行（`build_story_trajectory_details` 输出）。
#[derive(Debug, Clone)]
pub struct TrajectoryDetailRow {
    pub story: String,
    pub items: Vec<TrajectoryDetailItem>,
}

/// 关系对轨迹条目 / 行。
#[derive(Debug, Clone)]
pub struct PairTrajectoryItem {
    pub pair: String,
    pub summary: String,
    pub timeline: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PairTrajectoryRow {
    pub story: String,
    pub items: Vec<PairTrajectoryItem>,
}

/// 叙事轨迹行（drafts × consistency 合流）。
#[derive(Debug, Clone)]
pub struct NarrativeRow {
    pub story: String,
    pub summary: String,
    pub details: Vec<String>,
}

/// concept 节 `categories` 条目。
#[derive(Debug, Clone)]
pub struct ConceptCategory {
    pub category: String,
    pub cards: usize,
    pub warnings: usize,
    pub top_file: String,
    pub top_count: usize,
    pub top_kinds: String,
}

/// concept 节 dict。
#[derive(Debug, Clone)]
pub struct ConceptSection {
    pub exists: bool,
    pub files: usize,
    pub warnings: usize,
    pub summary_paths: Vec<PathBuf>,
    pub categories: Vec<ConceptCategory>,
}

/// plan 节 `plan_types` 条目。
#[derive(Debug, Clone)]
pub struct PlanTypeInfo {
    pub plan_type: String,
    pub files: usize,
    pub warnings: usize,
    pub top_file: String,
    pub top_count: usize,
    pub top_kinds: String,
}

/// plan 节 `story_trends` 条目。
#[derive(Debug, Clone)]
pub struct StoryTrend {
    pub story: String,
    pub chapter_flow: String,
    pub ending_flow: String,
    pub chapter_runs: Vec<String>,
    pub ending_runs: Vec<String>,
}

/// plan 节 dict。
#[derive(Debug, Clone)]
pub struct PlanSection {
    pub exists: bool,
    pub files: usize,
    pub warnings: usize,
    pub summary_paths: Vec<PathBuf>,
    pub plan_types: Vec<PlanTypeInfo>,
    pub chapter_function_distribution: String,
    pub ending_function_distribution: String,
    pub story_trends: Vec<StoryTrend>,
}

/// draft 节 `stories` 条目。
#[derive(Debug, Clone)]
pub struct DraftStory {
    pub story: String,
    pub chapters: usize,
    pub warnings: usize,
    pub top_file: String,
    pub top_count: usize,
    pub top_templates: String,
    pub top_fatigue: String,
    pub gate_summary: String,
    pub recommendation_summary: String,
    pub avg_axis: String,
    pub scene_summary: String,
    pub tone_summary: String,
    pub emotion_summary: String,
    pub speaker_summary: String,
    pub template_summary: String,
    pub alignment_summary: String,
    pub ending_signal_summary: String,
    pub alignment_mismatches: Vec<String>,
    pub voice_drifts: Vec<String>,
    pub chapter_runs: Vec<String>,
    pub ending_runs: Vec<String>,
    pub ending_signal_flow: String,
    pub ending_signal_runs: Vec<String>,
    pub trend_convergences: Vec<String>,
    pub tone_runs: Vec<String>,
    pub emotion_runs: Vec<String>,
    pub scorecard_summary_path: PathBuf,
    pub review_kit_summary_path: PathBuf,
    pub template_backlog_summary_path: PathBuf,
}

/// draft 节 `alignment_mismatches` 条目。
#[derive(Debug, Clone)]
pub struct DraftDriftSample {
    pub draft: String,
    pub chapter: String,
    pub ending: String,
    pub score: usize,
}

/// draft 节 dict。
#[derive(Debug, Clone)]
pub struct DraftSection {
    pub exists: bool,
    pub files: usize,
    pub warnings: usize,
    pub stories: Vec<DraftStory>,
    pub workspace_templates: String,
    pub template_targets: String,
    pub template_research_path: PathBuf,
    pub template_catalog_summary_path: PathBuf,
    pub template_catalog_json_path: PathBuf,
    pub template_learning_anchors: Vec<Value>,
    pub template_writeback_queue: Vec<Value>,
    pub alignment_summary: String,
    pub alignment_mismatches: Vec<DraftDriftSample>,
}

/// consistency 节 dict。
#[derive(Debug, Clone)]
pub struct ConsistencySection {
    pub db_path: PathBuf,
    pub feedback_path: PathBuf,
    pub feedback_entries: usize,
    pub feedback_decisions: Vec<(String, usize)>,
    pub feedback_facets: Vec<(String, usize)>,
    pub feedback_backlog: Vec<consistency::BacklogItem>,
    pub feedback_pending: usize,
    pub feedback_pending_samples: Vec<ConflictRow>,
    pub story_alignment: Vec<AlignmentRow>,
    pub story_tension: Vec<TensionRow>,
    pub story_goal_tension: Vec<GoalTensionRow>,
    pub story_relationship_tension: Vec<RelationshipTensionRow>,
    pub story_conflicts: Vec<ConflictRow>,
    pub story_trajectories: Vec<TrajectorySummaryRow>,
    pub story_trajectory_details: Vec<TrajectoryDetailRow>,
    pub relationship_pair_trajectories: Vec<PairTrajectoryRow>,
}

mod collect;
mod dashboard;
mod draft;
mod tools;
mod trajectories;

use collect::{collect_concept_section, collect_consistency_section, collect_plan_section};
use dashboard::build_dashboard;
use draft::collect_draft_section;
