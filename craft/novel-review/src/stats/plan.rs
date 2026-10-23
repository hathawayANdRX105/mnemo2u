//! `stats.plan`：arc/story/chapter plan 的镜像 markdown 统计报告。
//!
//! 每个 plan 文件生成一份镜像 `*-stats` 树内的单文件报告，每个源目录生成
//! `SUMMARY.md`（文件清单、优先处理、告警类型分布、功能分布/趋势、重复场面功能、角色功能信号）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::draft::Counter;
use crate::audit::plan as plan_audit;
use crate::config;
use crate::input;
use crate::stats::draft::path_from_parts;

/// `summarize_runs`：把连续同标签段（长度 ≥ `min_run`）压成字符串；
/// `unclear`/`missing` 段跳过；最多保留 `limit` 条。
#[must_use]
pub fn summarize_runs(labels: &[String], min_run: u32, limit: usize) -> Vec<String> {
    if labels.is_empty() {
        return Vec::new();
    }
    let mut runs: Vec<String> = Vec::new();
    let mut current = labels[0].clone();
    let mut start: u32 = 1;
    let mut length: u32 = 1;
    for (i, label) in labels[1..].iter().enumerate() {
        let index = (i as u32) + 2;
        if label == &current {
            length += 1;
            continue;
        }
        if !matches!(current.as_str(), "unclear" | "missing") && length >= min_run {
            runs.push(format!("{current} x{length} (#{start}-#{})", index - 1));
        }
        current = label.clone();
        start = index;
        length = 1;
    }
    if !matches!(current.as_str(), "unclear" | "missing") && length >= min_run {
        runs.push(format!(
            "{current} x{length} (#{start}-#{})",
            start + length - 1
        ));
    }
    runs.truncate(limit);
    runs
}

/// `stats_path_for`：把路径中首个计划目录名替换为 `*-stats`；
/// 有 `output_root` 时镜像为 `output_root/<小说目录名>/...`（首层计划目录直接落在根下）。
///
/// 计划目录名来自 `paths` 配置，不是内置常量。
pub fn stats_path_for(
    plan_path: &Path,
    output_root: Option<&Path>,
    paths: &crate::config::PathsConfig,
) -> Result<PathBuf> {
    // 路径 components 逐个转字符串（根组件为 "/"）。
    let parts: Vec<String> = plan_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(idx) = paths.plan_dir_component(plan_path) {
        let mut mirrored = parts.clone();
        mirrored[idx] = format!("{}-stats", parts[idx]);
        let stats_path = path_from_parts(&mirrored);
        return match output_root {
            None => Ok(stats_path),
            Some(root) => {
                if idx == 0 {
                    Ok(root.join(&stats_path))
                } else {
                    let novel_name = &parts[idx - 1];
                    let rel = mirrored[idx..].join("/");
                    Ok(root.join(novel_name).join(rel))
                }
            }
        };
    }
    anyhow::bail!(
        "Path does not live under a plan directory: {}",
        plan_path.display()
    );
}

pub fn collect_targets(raw: &[PathBuf]) -> Vec<PathBuf> {
    let mut targets = plan_audit::iter_targets(raw);
    targets.sort_by_key(|path| path.to_string_lossy().into_owned());
    targets
}

/// 单文件报告结果：(源路径, 检测出的 plan 类型, 警告列表)。
pub type Report = (PathBuf, String, Vec<plan_audit::Warning>);

pub fn build_single_reports(
    engine: &plan_audit::PlanEngine,
    files: &[PathBuf],
    output_root: Option<&Path>,
    single_output: Option<&Path>,
) -> Result<Vec<Report>> {
    let paths = engine.paths();
    let mut reports: Vec<Report> = Vec::new();
    for path in files {
        let (plan_type, warnings) = plan_audit::audit_file(engine, path)?;
        let report = plan_audit::format_report(path, &plan_type, &warnings);
        let out_path = match single_output {
            Some(single) => single.to_path_buf(),
            None => stats_path_for(path, output_root, paths)?,
        };
        input::write_text(&out_path, &format!("{report}\n"))?;
        reports.push((path.clone(), plan_type, warnings));
    }
    Ok(reports)
}

pub fn build_directory_summaries(
    engine: &plan_audit::PlanEngine,
    reports: &[Report],
    output_root: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let paths = engine.paths();
    let mut written: Vec<PathBuf> = Vec::new();
    // 分组：键=文件父目录，插入序=首现序。
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

    let chapter_rules = engine.chapter_function_rules();
    let ending_rules = engine.ending_function_rules();
    let ending_group: Vec<&str> = engine
        .chapter_ending_group()
        .iter()
        .map(String::as_str)
        .collect();

    for (source_dir, indices) in &grouped {
        let group: Vec<&Report> = indices.iter().map(|i| &reports[*i]).collect();
        let mut ordered: Vec<&Report> = group;
        ordered.sort_by_key(|item| item.0.file_name().unwrap_or_default());

        let mut warning_counter = Counter::default();
        let mut role_warning_count: usize = 0;
        for item in &ordered {
            for warning in &item.2 {
                warning_counter.add(&warning.kind);
            }
            if item.2.iter().any(|w| w.kind == "thin_role_functions") {
                role_warning_count += 1;
            }
        }

        let mut chapter_function_counter = Counter::default();
        let mut ending_function_counter = Counter::default();
        let mut scene_function_warning_counter = Counter::default();
        let mut chapter_flow: Vec<String> = Vec::new();
        let mut ending_flow: Vec<String> = Vec::new();

        for item in &ordered {
            let path = &item.0;
            let plan_type = &item.1;
            let warnings = &item.2;
            if plan_type == "chapter-plan" {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("无法读取大纲文件 {}", path.display()))?;
                let lines: Vec<&str> = text.lines().collect();
                let headings = plan_audit::parse_headings(&lines);
                let sections = plan_audit::collect_section_lines(&lines, &headings);

                let (_name, chapter_section) = plan_audit::find_section(&sections, &["本章功能"]);
                let chapter_function_text = {
                    let bullets = plan_audit::bullet_lines(chapter_section);
                    let joined = bullets
                        .iter()
                        .map(|line| line.1.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if joined.is_empty() {
                        plan_audit::section_text(chapter_section)
                    } else {
                        joined
                    }
                };
                let chapter_function =
                    plan_audit::detect_function_label(&chapter_function_text, chapter_rules);
                chapter_function_counter.add(&chapter_function);
                chapter_flow.push(chapter_function);

                let (_ending_name, ending_section) =
                    plan_audit::find_section(&sections, &ending_group);
                let ending_text = {
                    let bullets = plan_audit::bullet_lines(ending_section);
                    let joined = bullets
                        .iter()
                        .map(|line| line.1.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if joined.is_empty() {
                        plan_audit::section_text(ending_section)
                    } else {
                        joined
                    }
                };
                let ending_function = plan_audit::detect_function_label(&ending_text, ending_rules);
                ending_function_counter.add(&ending_function);
                ending_flow.push(ending_function);

                for warning in warnings {
                    if warning.kind == "scene_function_monotony" {
                        let key = if warning.snippet.is_empty() {
                            &warning.message
                        } else {
                            &warning.snippet
                        };
                        scene_function_warning_counter.add(key);
                    }
                }
            }
        }

        let mut summary_lines: Vec<String> = vec![
            "# SUMMARY".to_string(),
            String::new(),
            "## Files".to_string(),
        ];
        for item in &ordered {
            let status = if item.2.is_empty() { "OK" } else { "WARN" };
            summary_lines.push(format!(
                "- `{}` type=`{}` status=`{}` warnings=`{}`",
                item.0
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                item.1,
                status,
                item.2.len()
            ));
        }
        summary_lines.push(String::new());

        summary_lines.push("## Priority".to_string());
        let mut by_priority = ordered.clone();
        by_priority.sort_by(|a, b| {
            b.2.len().cmp(&a.2.len()).then_with(|| {
                a.0.file_name()
                    .unwrap_or_default()
                    .cmp(b.0.file_name().unwrap_or_default())
            })
        });
        for item in by_priority.iter().take(5) {
            let mut top_counter = Counter::default();
            for warning in &item.2 {
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
                "- `{}` type=`{}` warnings=`{}` top=`{}`",
                item.0
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                item.1,
                item.2.len(),
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
        summary_lines.push(String::new());

        if !chapter_function_counter.is_empty() {
            summary_lines.push("## Chapter Function Distribution".to_string());
            for (name, count) in chapter_function_counter.most_common_all() {
                summary_lines.push(format!("- `{name}` x{count}"));
            }
            summary_lines.push(String::new());
        }

        if !ending_function_counter.is_empty() {
            summary_lines.push("## Ending Function Distribution".to_string());
            for (name, count) in ending_function_counter.most_common_all() {
                summary_lines.push(format!("- `{name}` x{count}"));
            }
            summary_lines.push(String::new());
        }

        let chapter_runs = summarize_runs(&chapter_flow, 3, 6);
        let ending_runs = summarize_runs(&ending_flow, 3, 6);
        if !chapter_runs.is_empty() || !ending_runs.is_empty() {
            summary_lines.push("## Function Trend Signals".to_string());
            for item in &chapter_runs {
                summary_lines.push(format!("- `chapter_run` {item}"));
            }
            for item in &ending_runs {
                summary_lines.push(format!("- `ending_run` {item}"));
            }
            summary_lines.push(String::new());
        }

        if !scene_function_warning_counter.is_empty() {
            summary_lines.push("## Repeated Scene Function Signals".to_string());
            for (name, count) in scene_function_warning_counter.most_common(8) {
                summary_lines.push(format!("- `{name}` x{count}"));
            }
            summary_lines.push(String::new());
        }

        if role_warning_count > 0 {
            summary_lines.push("## Role Function Signals".to_string());
            summary_lines.push(format!(
                "- `thin_role_functions` x{role_warning_count}：这些 Story 的“主要角色与功能”更像点名名单，缺少谁在推动/阻拦/见证/施压。"
            ));
        }

        let summary_path = stats_path_for(&source_dir.join("SUMMARY.md"), output_root, paths)?;
        input::write_text(&summary_path, &(summary_lines.join("\n") + "\n"))?;
        written.push(summary_path);
    }

    Ok(written)
}

/// `stats-plan` 全流程，返回退出码。
///
/// `output` 与 `output_root` 互斥；`output` 仅在恰好收集到一个 plan 文件时可用。
pub fn run(
    paths: &[PathBuf],
    inputs: &[PathBuf],
    output: Option<&Path>,
    output_root: Option<&Path>,
    rules: Option<&Path>,
) -> Result<i32> {
    if output.is_some() && output_root.is_some() {
        anyhow::bail!("Use either --output or --output-root, not both.");
    }
    let cfg = config::load_rules_from(rules)?;
    let engine = plan_audit::PlanEngine::new(&cfg.plan, &cfg.paths)?;
    let raw = input::resolve_inputs(paths, inputs)?;
    let files = collect_targets(&raw);
    if files.is_empty() {
        anyhow::bail!("No plan files found.");
    }
    if let Some(single_output) = output {
        if files.len() != 1 {
            anyhow::bail!("--output requires exactly one collected plan file.");
        }
        build_single_reports(&engine, &files, None, Some(single_output))?;
        return Ok(0);
    }
    let reports = build_single_reports(&engine, &files, output_root, None)?;
    build_directory_summaries(&engine, &reports, output_root)?;
    Ok(0)
}
