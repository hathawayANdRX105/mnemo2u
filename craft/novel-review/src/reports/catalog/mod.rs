//! 工作区级模板/词项候选目录（`reports-catalog` 子命令）。
//!
//! 输入为 novel 目录（读已有 `draft-stats/*/template-backlog/CANDIDATES.json`）
//! 或草稿章节文件（现跑分析并按 story 写 backlog 两件套），聚合后生成
//! `draft-stats/template-catalog/SUMMARY.md` 与 `CATALOG.json`（字节对齐）。
//!
//! 子模块：`aggregate`（跨 story 聚合、学习锚点、回写队列），
//! `report`（库名查表与 CATALOG.json / SUMMARY.md 构建）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use glob::glob;
use serde_json::{json, Map, Value};

use crate::audit::draft::{analyze_path, build_corpus_profile, Analysis, DraftContext};
use crate::config::{self, RegexRule, TemplateRule, TrackedTerm};
use crate::input::{write_json, write_text};
use crate::reports::backlog;
use crate::rules::build_template_bank;
use crate::stats::draft::{collect_chapter_files, Ctr};

/// `reports-catalog` 子命令参数（仅位置 `paths` nargs+）。
#[derive(Debug, Clone)]
pub struct CatalogOptions {
    /// 产物根目录；缺省写在输入树旁。
    pub output_root: Option<PathBuf>,
    /// novel 目录、draft 目录或草稿章节文件。
    pub paths: Vec<PathBuf>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
}

/// `summary_path_for(novel_dir)`：`novel_dir/draft-stats/template-catalog/SUMMARY.md`。
pub fn summary_path_for(novel_dir: &Path) -> PathBuf {
    novel_dir
        .join("draft-stats")
        .join("template-catalog")
        .join("SUMMARY.md")
}

/// `json_path_for(novel_dir)`：`novel_dir/draft-stats/template-catalog/CATALOG.json`。
pub fn json_path_for(novel_dir: &Path) -> PathBuf {
    novel_dir
        .join("draft-stats")
        .join("template-catalog")
        .join("CATALOG.json")
}

/// `novel_dir_for_story_dir(story_dir)`：最近的正文目录祖先之父目录。
fn novel_dir_for_story_dir(
    story_dir: &Path,
    paths: &crate::config::PathsConfig,
) -> Option<PathBuf> {
    for parent in story_dir.ancestors().skip(1) {
        if paths.drafts_root_component(parent).is_some() {
            // pathlib join 语义：`Path('.') / "draft-stats"` 折叠为 `draft-stats`，
            // 故 drafts 位于 cwd 根时 novel 根用空路径（join 不产生 `./` 前缀）。
            let novel = parent.parent().map_or_else(PathBuf::new, PathBuf::from);
            return Some(if novel.as_os_str() == "." {
                PathBuf::new()
            } else {
                novel
            });
        }
    }
    None
}

/// `build_story_payloads`：逐章分析（sample_limit=6）→ 按 story 写
/// backlog 两件套 → 收集 CANDIDATES 载荷。novel 目录无法解析时报 Err（调用方
/// 打印 `Failed to resolve novel directory from draft paths.` 并退 1）。
fn build_story_payloads(
    files: &[PathBuf],
    ctx: &DraftContext,
    template_bank: &[TemplateRule],
    term_bank: &[TrackedTerm],
    output_root: Option<&Path>,
) -> Result<(Option<PathBuf>, Vec<Value>)> {
    let corpus_profile = build_corpus_profile(ctx, &ctx.corpus_paths_for_targets(files), files)?;

    let mut novel_dir: Option<PathBuf> = None;
    let mut analyses: Vec<(PathBuf, Analysis)> = Vec::new();
    for draft_path in files {
        if novel_dir.is_none() {
            if let Some(story_dir) = draft_path.parent() {
                novel_dir = novel_dir_for_story_dir(story_dir, ctx.paths());
            }
        }
        let analysis = analyze_path(
            ctx,
            draft_path,
            template_bank,
            term_bank,
            corpus_profile.as_ref(),
            6,
        )
        .with_context(|| format!("无法分析章节 {}", draft_path.display()))?;
        analyses.push((draft_path.clone(), analysis));
    }

    // novel_dir 无法解析时返回 None（由 resolve_payloads 打印固定文案并退 1）。

    // 按 story 目录分组（首现序），再按 story 目录字典序处理。
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
    if novel_dir.is_none() {
        // 错误发生在
        // 写任何 backlog 文件之前。
        return Ok((None, Vec::new()));
    }

    let mut payloads: Vec<Value> = Vec::new();
    for (story_dir, indexes) in &groups {
        let items: Vec<(PathBuf, &Analysis)> = indexes
            .iter()
            .map(|&i| (analyses[i].0.clone(), &analyses[i].1))
            .collect();
        let (markdown, payload) = backlog::build_story_backlog(story_dir, &items)?;
        let layout = ctx.paths();
        let summary = backlog::backlog_path_for(&items[0].0, layout, output_root)?;
        let candidates = backlog::candidates_path_for(&items[0].0, layout, output_root)?;
        write_text(&summary, &markdown)?;
        write_json(&candidates, &payload)?;
        payloads.push(payload);
    }
    Ok((novel_dir, payloads))
}

/// `load_story_payloads_from_stats`：按排序读全部
/// `draft-stats/arc*/story*/template-backlog/CANDIDATES.json`（仅 JSON object 有效）。
fn load_story_payloads_from_stats(novel_dir: &Path) -> Vec<Value> {
    let pattern = format!(
        "{}/draft-stats/arc*/story*/template-backlog/CANDIDATES.json",
        novel_dir.display()
    );
    let mut paths: Vec<PathBuf> = glob(&pattern)
        .map(|paths| paths.filter_map(Result::ok).collect::<Vec<_>>())
        .unwrap_or_default();
    paths.sort();
    let mut payloads: Vec<Value> = Vec::new();
    for path in paths {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let payload: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if payload.is_object() {
            payloads.push(payload);
        }
    }
    payloads
}

/// `resolve_payloads`。
fn resolve_payloads(
    paths: &[PathBuf],
    ctx: &DraftContext,
    template_bank: &[TemplateRule],
    term_bank: &[TrackedTerm],
    output_root: Option<&Path>,
) -> Result<Option<(PathBuf, Vec<Value>)>> {
    if paths.len() == 1 {
        let path = &paths[0];
        if path.is_dir() && path.join("draft-stats").exists() {
            let novel_dir = path.clone();
            let payloads = load_story_payloads_from_stats(&novel_dir);
            if !payloads.is_empty() {
                return Ok(Some((novel_dir, payloads)));
            }
        }
    }
    let files = collect_chapter_files(paths)?;
    if files.is_empty() {
        eprintln!("No draft chapter files found.");
        return Ok(None);
    }
    let (novel_dir, payloads) =
        build_story_payloads(&files, ctx, template_bank, term_bank, output_root)?;
    match novel_dir {
        Some(dir) => Ok(Some((dir, payloads))),
        None => {
            eprintln!("Failed to resolve novel directory from draft paths.");
            Ok(None)
        }
    }
}

/// 解析载荷 → 聚合 → 写 SUMMARY.md + CATALOG.json。
/// 返回（退出码, 应打印路径序列）。
pub fn run(opts: &CatalogOptions) -> Result<(i32, Vec<PathBuf>)> {
    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let ctx = DraftContext::new(rules.clone())?;
    let template_bank = build_template_bank(ctx.draft_rules());
    let term_bank = rules.draft.tracked_terms.clone();

    let (novel_dir, payloads) = match resolve_payloads(
        &opts.paths,
        &ctx,
        &template_bank,
        &term_bank,
        opts.output_root.as_deref(),
    )? {
        Some(pair) => pair,
        None => return Ok((1, Vec::new())),
    };
    if payloads.is_empty() {
        eprintln!("No story candidate payloads found.");
        return Ok((1, Vec::new()));
    }

    let catalog = build_catalog_payload(&ctx, &novel_dir, &payloads);
    let arr = |key: &str| -> Vec<Value> {
        catalog
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let template_candidates = arr("template_candidates");
    let template_families = arr("template_families");
    let term_candidates = arr("term_candidates");
    let keep_candidates = arr("keep_candidates");
    let deposition_targets = arr("deposition_targets");
    let learning_anchors = arr("learning_anchors");
    let writeback_queue = arr("writeback_queue");
    let sections = CatalogSections {
        template_candidates: &template_candidates,
        template_families: &template_families,
        term_candidates: &term_candidates,
        keep_candidates: &keep_candidates,
        deposition_targets: &deposition_targets,
        learning_anchors: &learning_anchors,
        writeback_queue: &writeback_queue,
    };
    let markdown = build_catalog_markdown(&novel_dir, payloads.len(), &sections);
    let summary_path = summary_path_for(&novel_dir);
    let catalog_path = json_path_for(&novel_dir);
    write_text(&summary_path, &markdown)?;
    write_json(&catalog_path, &catalog)?;
    Ok((0, vec![summary_path, catalog_path]))
}

mod aggregate;
mod report;

pub use report::{build_catalog_markdown, build_catalog_payload, CatalogSections};
