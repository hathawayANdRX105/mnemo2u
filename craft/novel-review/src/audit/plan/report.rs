//! plan 报告渲染与输出：markdown/JSON 报告、目录遍历、CLI 全流程。
//!
//! - 报告渲染（markdown）为逐字节稳定输出；
//! - 正则统一 `fancy_regex`；`ignore_case` 规则在 pattern 前缀 `(?i)`。

use super::*;
/// `format_report`：markdown 报告（输出逐字节稳定；`path.name` 为最后一级路径）。
pub fn format_report(path: &Path, plan_type: &str, warnings: &[Warning]) -> String {
    let status = if warnings.is_empty() { "OK" } else { "WARN" };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mut lines = vec![
        format!("# {name}"),
        String::new(),
        format!("- type: `{plan_type}`"),
        format!("- status: `{status}`"),
        format!("- warnings: `{}`", warnings.len()),
        String::new(),
    ];
    if warnings.is_empty() {
        lines.push("无警告。".to_string());
        return lines.join("\n");
    }
    lines.push("## Warnings".to_string());
    for warning in warnings {
        let location = if warning.line_no > 0 {
            format!("L{}", warning.line_no)
        } else {
            "global".to_string()
        };
        lines.push(format!(
            "- `{location}` `{}` {}",
            warning.kind, warning.message
        ));
        if !warning.snippet.is_empty() {
            lines.push(format!("  - `{}`", warning.snippet));
        }
    }
    lines.join("\n")
}

/// JSON 报告单篇结构（`type` 为 JSON 键名）。
#[derive(Debug, Clone, PartialEq, Serialize)]
struct JsonReport {
    source: String,
    #[serde(rename = "type")]
    plan_type: String,
    status: String,
    warnings: Vec<Warning>,
}

/// `iter_targets`：目录递归找 `*.md`（跳过 readme.md，按路径排序）；文件直接入列。
pub fn iter_targets(raw_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = Vec::new();
    for raw in raw_paths {
        if raw.is_dir() {
            let mut found: Vec<PathBuf> = Vec::new();
            walk_md_dir(raw, &mut found);
            found.sort();
            for path in found {
                let is_readme = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase() == "readme.md");
                if !is_readme {
                    targets.push(path);
                }
            }
        } else if raw.is_file() {
            let is_readme = raw
                .file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase() == "readme.md");
            if !is_readme {
                targets.push(raw.clone());
            }
        }
    }
    targets
}

fn walk_md_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.is_dir() {
            subdirs.push(path);
        } else if meta.is_file() && entry.file_name().to_string_lossy().ends_with(".md") {
            out.push(path);
        }
    }
    subdirs.sort();
    for sub in subdirs {
        walk_md_dir(&sub, out);
    }
}

fn json_reports(reports: &[(PathBuf, String, Vec<Warning>)]) -> Result<String> {
    let payload: Vec<JsonReport> = reports
        .iter()
        .map(|(path, plan_type, warnings)| JsonReport {
            source: path.display().to_string(),
            plan_type: plan_type.clone(),
            status: if warnings.is_empty() { "OK" } else { "WARN" }.to_string(),
            warnings: warnings.clone(),
        })
        .collect();
    Ok(serde_json::to_string_pretty(&payload)?)
}

fn write_reports(
    reports: &[(PathBuf, String, Vec<Warning>)],
    format: OutputFormat,
    output: Option<&Path>,
) -> Result<()> {
    match output {
        None => match format {
            OutputFormat::Json => {
                let json = json_reports(reports)?;
                println!("{json}");
            }
            OutputFormat::Text | OutputFormat::Markdown => {
                for (path, plan_type, warnings) in reports {
                    println!("{}", format_report(path, plan_type, warnings));
                    println!();
                }
            }
        },
        Some(out_path) => match format {
            OutputFormat::Json => {
                let json = json_reports(reports)?;
                input::write_json_line(out_path, &json)?;
            }
            OutputFormat::Text | OutputFormat::Markdown => {
                let suffix = if format == OutputFormat::Markdown {
                    ".md"
                } else {
                    ".txt"
                };
                if reports.len() == 1 {
                    let (path, plan_type, warnings) = &reports[0];
                    let report = format_report(path, plan_type, warnings);
                    input::write_text(out_path, &format!("{report}\n"))?;
                } else {
                    fs::create_dir_all(out_path)
                        .with_context(|| format!("无法创建目录 {}", out_path.display()))?;
                    for (path, plan_type, warnings) in reports {
                        let stem = path
                            .file_stem()
                            .map(|s| s.to_string_lossy())
                            .unwrap_or_default();
                        let report = format_report(path, plan_type, warnings);
                        input::write_text(
                            &out_path.join(format!("{stem}{suffix}")),
                            &format!("{report}\n"),
                        )?;
                    }
                }
            }
        },
    }
    Ok(())
}

/// `main` 全流程：解析输入 → 加载配置 → 逐文件审计 → 写报告。返回退出码。
pub fn run(
    paths: &[PathBuf],
    inputs: &[PathBuf],
    format: OutputFormat,
    fail_on_warn: bool,
    output: Option<&Path>,
    rules: Option<&Path>,
) -> Result<i32> {
    let raw = input::resolve_inputs(paths, inputs)?;
    let targets = iter_targets(&raw);
    if targets.is_empty() {
        anyhow::bail!("No plan files found.");
    }
    let cfg = config::load_rules_from(rules)?;
    let engine = PlanEngine::new(&cfg.plan, &cfg.paths)?;

    let mut reports: Vec<(PathBuf, String, Vec<Warning>)> = Vec::new();
    let mut warned = false;
    for path in &targets {
        let (plan_type, warnings) = audit_file(&engine, path)?;
        if !warnings.is_empty() {
            warned = true;
        }
        reports.push((path.clone(), plan_type, warnings));
    }
    write_reports(&reports, format, output)?;
    Ok(if fail_on_warn && warned { 1 } else { 0 })
}
