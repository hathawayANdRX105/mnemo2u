//! 跨 Story 模板积压（`reports-backlog` 子命令）。
//!
//! 对每章跑草稿分析（缺省 `--sample-limit 6`），按 story 目录分组，
//! 生成 `template-backlog/SUMMARY.md`（markdown 逐行稳定）与
//! `template-backlog/CANDIDATES.json`（2 空格缩进 + 末尾换行）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::audit::draft::{analyze_path, build_corpus_profile, Analysis, DraftContext};
use crate::config;
use crate::input::write_json;
use crate::input::write_text;
use crate::reports::learning::{collect_rule_suggestions, collect_template_backlog};
use crate::reports::scorecard;
use crate::rules::build_template_bank;
use crate::stats::draft::{chapter_sort_key, collect_chapter_files, stats_path_for, Ctr};

/// `reports-backlog` 子命令参数（位置 `paths` nargs+、
/// `--sample-limit` 缺省 6）。
#[derive(Debug, Clone)]
pub struct BacklogOptions {
    /// 产物根目录；缺省写在输入树旁。
    pub output_root: Option<PathBuf>,
    /// 草稿章文件或目录。
    pub paths: Vec<PathBuf>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// 每条规则最多记录的样本行数（默认 6）。
    pub sample_limit: usize,
}

/// `backlog_path_for(draft)`：`stats_path_for(draft).parent / template-backlog / SUMMARY.md`。
pub fn backlog_path_for(
    draft_path: &Path,
    paths: &crate::config::PathsConfig,
    output_root: Option<&Path>,
) -> Result<PathBuf> {
    let stats = stats_path_for(draft_path, output_root, paths)?;
    Ok(stats
        .parent()
        .context("stats 路径无父目录")?
        .join("template-backlog")
        .join("SUMMARY.md"))
}

/// `candidates_path_for(draft)`：`stats_path_for(draft).parent / template-backlog / CANDIDATES.json`。
pub fn candidates_path_for(
    draft_path: &Path,
    paths: &crate::config::PathsConfig,
    output_root: Option<&Path>,
) -> Result<PathBuf> {
    let stats = stats_path_for(draft_path, output_root, paths)?;
    Ok(stats
        .parent()
        .context("stats 路径无父目录")?
        .join("template-backlog")
        .join("CANDIDATES.json"))
}

/// 模板候选（`infer_template_candidate`：按 `bucket::name` 拆键）。
fn infer_template_candidate(key: &str, count: usize, sample: &str) -> Value {
    let (bucket, name) = key
        .split_once("::")
        .map(|(b, n)| (b.to_string(), n.to_string()))
        .unwrap_or_else(|| (key.to_string(), String::new()));
    json!({
        "bucket": bucket,
        "name": name,
        "count": count,
        "sample": sample,
        "suggested_target": "configs/rules/review.yaml#draft.template_rules",
        "reason": "跨章重复出现，优先作为模板库候选继续人工筛选。",
    })
}

/// 词项候选（`infer_term_candidate`：强制 bucket=`learned_filter`）。
fn infer_term_candidate(key: &str, count: usize, sample: &str) -> Value {
    let name = key
        .split_once("::")
        .map(|(_, n)| n.to_string())
        .unwrap_or_default();
    json!({
        "bucket": "learned_filter",
        "name": name,
        "count": count,
        "sample": sample,
        "suggested_target": "configs/rules/review.yaml#draft.tracked_terms",
        "reason": "跨章重复出现，更像词项或短语，需要进词库观察。",
    })
}

/// 保留候选（`infer_keep_candidate`）。
fn infer_keep_candidate(name: &str, count: usize, reason: &str) -> Value {
    json!({
        "name": name,
        "count": count,
        "reason": reason,
        "suggested_target": "configs/rules/review.yaml#draft.template_rules",
        "action": "designed_keep_review",
    })
}

/// `build_story_backlog`：markdown + 候选 JSON（键序固定）。
pub fn build_story_backlog(
    story_dir: &Path,
    analyses: &[(PathBuf, &Analysis)],
) -> Result<(String, Value)> {
    let mut template_counter: Ctr = Ctr::default();
    let mut template_samples: HashMap<String, String> = HashMap::new();
    let mut bonus_counter: Ctr = Ctr::default();
    let mut bonus_reasons: HashMap<String, String> = HashMap::new();
    let mut bank_target_counter: Ctr = Ctr::default();

    let mut ordered: Vec<&(PathBuf, &Analysis)> = analyses.iter().collect();
    ordered.sort_by_key(|(path, _)| chapter_sort_key(path));

    for (_path, analysis) in ordered {
        for item in collect_template_backlog(analysis) {
            let key = format!("{}::{}", item.bucket, item.name);
            template_counter.add(&key, 1);
            if !template_samples.contains_key(&key) && !item.sample.is_empty() {
                template_samples.insert(key, item.sample);
            }
        }
        for item in scorecard::build_bonus_candidates(analysis) {
            bonus_counter.add(&item.name, 1);
            bonus_reasons
                .entry(item.name.clone())
                .or_insert(item.reason);
        }
        for suggestion in collect_rule_suggestions(analysis) {
            bank_target_counter.add(&suggestion.target, 1);
        }
    }

    let mut lines: Vec<String> = vec!["# Template Backlog".to_string(), String::new()];
    lines.push(format!("- story: `{}`", story_dir.display()));
    lines.push(format!("- chapters: `{}`", analyses.len()));
    lines.push(String::new());

    lines.push("## Repeat Candidates".to_string());
    if !template_counter.is_empty() {
        for (name, count) in template_counter.most_common(16) {
            lines.push(format!("- `{name}` x{count}"));
            let sample = template_samples.get(&name).cloned().unwrap_or_default();
            if !sample.is_empty() {
                lines.push(format!("  样例：{sample}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Keep Candidates".to_string());
    if !bonus_counter.is_empty() {
        for (name, count) in bonus_counter.most_common(12) {
            let reason = bonus_reasons.get(&name).cloned().unwrap_or_default();
            lines.push(format!("- `{name}` x{count}：{reason}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Deposition Targets".to_string());
    if !bank_target_counter.is_empty() {
        for (name, count) in bank_target_counter.most_common_all() {
            lines.push(format!("- `{name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Next Actions".to_string());
    lines.push(
        "1. 先看 `Repeat Candidates` 里跨章反复出现的家族，判断它该进模板库、词库，还是只算局部问题。"
            .to_string(),
    );
    lines.push(
        "2. 再看 `Keep Candidates`，避免把本来应保留的节奏、章末收束或动作后果误杀。".to_string(),
    );
    lines.push(
        "3. 最后按 `Deposition Targets` 决定写回 `configs/rules/review.yaml`（`draft.template_rules` / `draft.tracked_terms`）、`skills/review-guide.md` 还是本书规则。"
            .to_string(),
    );
    lines.push(String::new());

    let mut template_bank_candidates: Vec<Value> = Vec::new();
    let mut term_bank_candidates: Vec<Value> = Vec::new();
    let mut keep_candidates: Vec<Value> = Vec::new();

    for (key, count) in template_counter.most_common(16) {
        let sample = template_samples.get(&key).cloned().unwrap_or_default();
        if key.starts_with("learned_filter::") || key.starts_with("tracked_term::") {
            term_bank_candidates.push(infer_term_candidate(&key, count, &sample));
        } else {
            template_bank_candidates.push(infer_template_candidate(&key, count, &sample));
        }
    }

    for (name, count) in bonus_counter.most_common(12) {
        let reason = bonus_reasons.get(&name).cloned().unwrap_or_default();
        keep_candidates.push(infer_keep_candidate(&name, count, &reason));
    }

    let payload = {
        let mut map = Map::new();
        map.insert(
            "story".to_string(),
            json!(story_dir.to_string_lossy().into_owned()),
        );
        map.insert("chapters".to_string(), json!(analyses.len()));
        map.insert(
            "template_bank_candidates".to_string(),
            Value::Array(template_bank_candidates),
        );
        map.insert(
            "term_bank_candidates".to_string(),
            Value::Array(term_bank_candidates),
        );
        map.insert("keep_candidates".to_string(), Value::Array(keep_candidates));
        map.insert(
            "deposition_targets".to_string(),
            Value::Array(
                bank_target_counter
                    .most_common_all()
                    .into_iter()
                    .map(|(target, count)| json!({"target": target, "count": count}))
                    .collect(),
            ),
        );
        Value::Object(map)
    };

    Ok((lines.join("\n") + "\n", payload))
}

/// 收集章节 → 分析 → 按 story 写 backlog 两件套。
/// 返回（退出码, 应打印路径序列）。
pub fn run(opts: &BacklogOptions) -> Result<(i32, Vec<PathBuf>)> {
    let files = collect_chapter_files(&opts.paths)?;
    if files.is_empty() {
        eprintln!("No draft chapter files found.");
        return Ok((1, Vec::new()));
    }

    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let ctx = DraftContext::new(rules)?;
    let template_bank = build_template_bank(ctx.draft_rules());
    let corpus_profile = build_corpus_profile(&ctx, &ctx.corpus_paths_for_targets(&files), &files)?;

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
        let (markdown, payload) = build_story_backlog(story_dir, &items)?;
        let layout = ctx.paths();
        let out_path = backlog_path_for(&items[0].0, layout, opts.output_root.as_deref())?;
        let json_path = candidates_path_for(&items[0].0, layout, opts.output_root.as_deref())?;
        write_text(&out_path, &markdown)?;
        write_json(&json_path, &payload)?;
        printed.push(out_path);
        printed.push(json_path);
    }
    Ok((0, printed))
}
