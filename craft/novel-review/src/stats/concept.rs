//! `stats.concept`：概念卡镜像 markdown 统计报告。
//!
//! 每张卡生成一份 `card-stats` 镜像树内的单文件报告，每个源目录生成 `SUMMARY.md`。

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::audit::concept as concept_audit;
use crate::audit::draft::Counter;
use crate::input;
use crate::stats::draft::path_from_parts;

/// `stats_path_for`：把路径中首个 `cards` 目录名替换为 `card-stats`（无 output_root 参数）。
pub fn stats_path_for(card_path: &Path) -> Result<PathBuf> {
    // 路径 components 逐个转字符串（根组件为 "/"）。
    let mut parts: Vec<String> = card_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    for (idx, part) in parts.iter().enumerate() {
        if part.as_str() == "cards" {
            parts[idx] = "card-stats".to_string();
            return Ok(path_from_parts(&parts));
        }
    }
    anyhow::bail!(
        "Path does not live under concept/cards: {}",
        card_path.display()
    );
}

pub fn collect_targets(raw_paths: &[PathBuf], include_templates: bool) -> Vec<PathBuf> {
    let mut targets = concept_audit::iter_targets(raw_paths, include_templates);
    targets.sort_by_key(|path| path.to_string_lossy().into_owned());
    targets
}

/// 单卡报告结果：(源路径, 警告列表)。
pub type Report = (PathBuf, Vec<concept_audit::Warning>);

pub fn build_single_reports(files: &[PathBuf]) -> Result<Vec<Report>> {
    let mut reports: Vec<Report> = Vec::new();
    for path in files {
        let warnings = concept_audit::audit_card(path)?;
        let out_path = stats_path_for(path)?;
        let report = concept_audit::format_report(path, &warnings);
        // 不追加尾换行（与 plan 报告不同）。
        input::write_text(&out_path, &report)?;
        reports.push((path.clone(), warnings));
    }
    Ok(reports)
}

pub fn build_directory_summaries(reports: &[Report]) -> Result<Vec<PathBuf>> {
    let mut written: Vec<PathBuf> = Vec::new();
    // 分组：键=卡文件父目录，插入序=首现序。
    let mut grouped: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for (i, item) in reports.iter().enumerate() {
        let parent = item
            .0
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(PathBuf::new);
        match grouped.iter_mut().find(|(dir, _)| dir == &parent) {
            Some(group) => group.1.push(i),
            None => grouped.push((parent, vec![i])),
        }
    }

    for (source_dir, indices) in &grouped {
        let group: Vec<&Report> = indices.iter().map(|i| &reports[*i]).collect();
        let mut ordered: Vec<&Report> = group;
        ordered.sort_by_key(|item| item.0.file_name().unwrap_or_default());

        let mut warning_counter = Counter::default();
        for item in &ordered {
            for warning in &item.1 {
                warning_counter.add(&warning.kind);
            }
        }

        let mut summary_lines: Vec<String> = vec![
            "# SUMMARY".to_string(),
            String::new(),
            "## Cards".to_string(),
        ];
        for item in &ordered {
            let status = if item.1.is_empty() { "OK" } else { "WARN" };
            summary_lines.push(format!(
                "- `{}` status=`{}` warnings=`{}`",
                item.0
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                status,
                item.1.len()
            ));
        }
        summary_lines.push(String::new());

        summary_lines.push("## Priority".to_string());
        let mut by_priority = ordered.clone();
        by_priority.sort_by(|a, b| {
            b.1.len().cmp(&a.1.len()).then_with(|| {
                a.0.file_name()
                    .unwrap_or_default()
                    .cmp(b.0.file_name().unwrap_or_default())
            })
        });
        for item in by_priority.iter().take(5) {
            let mut top_counter = Counter::default();
            for warning in &item.1 {
                top_counter.add(&warning.kind);
            }
            let top_kinds = if top_counter.is_empty() {
                "无".to_string()
            } else {
                top_counter
                    .most_common(3)
                    .iter()
                    .map(|(kind, count)| format!("{kind} x{count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            summary_lines.push(format!(
                "- `{}` warnings=`{}` top=`{}`",
                item.0
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                item.1.len(),
                top_kinds
            ));
        }
        summary_lines.push(String::new());

        summary_lines.push("## Warning Kinds".to_string());
        if !warning_counter.is_empty() {
            for (kind, count) in warning_counter.most_common_all() {
                summary_lines.push(format!("- `{kind}` x{count}"));
            }
        } else {
            summary_lines.push("- 无".to_string());
        }

        // SUMMARY 不追加尾换行。
        let summary_path = stats_path_for(&source_dir.join("SUMMARY.md"))?;
        input::write_text(&summary_path, &summary_lines.join("\n"))?;
        written.push(summary_path);
    }

    Ok(written)
}

/// `stats-concept` 全流程，返回退出码。
pub fn run(paths: &[PathBuf], include_templates: bool) -> Result<i32> {
    let files = collect_targets(paths, include_templates);
    if files.is_empty() {
        anyhow::bail!("No concept cards found.");
    }
    let reports = build_single_reports(&files)?;
    build_directory_summaries(&reports)?;
    Ok(0)
}
