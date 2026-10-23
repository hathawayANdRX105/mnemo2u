//! 概念卡（concept card）审查：扫描概念卡目录，输出分类规则告警与 markdown 报告。
//!
//! 顺序语义：
//! - `CATEGORY_RULES` 为源码内硬编码分类规则表，键序/字段序按字面量声明顺序保持；
//! - 字段/小节「同键覆盖」用按位置替换的 Vec 模拟（首现序 + 末值）；
//! - 报告渲染（markdown）为逐字节稳定输出。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use fancy_regex::Regex;
use serde::Serialize;

/// 概念卡审查警告（`Warning` 结构体，字段序固定）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    pub line_no: u32,
    pub kind: String,
    pub message: String,
    pub snippet: String,
}

fn warn(line_no: u32, kind: &str, message: &str, snippet: &str) -> Warning {
    Warning {
        line_no,
        kind: kind.to_string(),
        message: message.to_string(),
        snippet: snippet.to_string(),
    }
}

static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"^(#{1,6})\s+(.+?)\s*$").unwrap());
static FIELD_RE: LazyLock<Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"^\s*-\s*([^：:]+)[：:]\s*(.*)$").unwrap());

/// 公共必备字段。
const COMMON_FIELDS: &[&str] = &["状态", "所属分类", "卡片 ID", "别名 / 英文名", "关联卡片"];

/// 单个分类目录的审查规则（`CATEGORY_RULES` 表的一个条目）。
struct CategoryRule {
    name: &'static str,
    allowed_categories: &'static [&'static str],
    required_fields: &'static [&'static str],
    required_headings: &'static [&'static str],
    required_heading_fields: &'static [(&'static str, &'static [&'static str])],
    id_prefix: &'static str,
}

/// 分类规则表，顺序即告警检查顺序（`required_heading_fields` 的迭代序影响告警序）。
const CATEGORY_RULES: &[CategoryRule] = &[
    CategoryRule {
        name: "characters",
        allowed_categories: &["角色", "characters"],
        required_fields: &["首次生效", "年龄", "当前位置", "时间线状态"],
        required_headings: &[
            "身份",
            "外形",
            "性格",
            "能力",
            "关系",
            "视角与信息边界",
            "写作约束",
        ],
        required_heading_fields: &[
            ("身份", &["身份", "阵营", "社会位置"]),
            ("外形", &["外形标签", "场面印象"]),
            ("性格", &["性格关键词", "行为习惯", "对话习惯"]),
            ("能力", &["能力", "触发条件", "限制"]),
            ("关系", &["当前关系状态"]),
            (
                "视角与信息边界",
                &["当前能认知什么", "当前不能直接知道什么", "称谓规则"],
            ),
        ],
        id_prefix: "CHR-",
    },
    CategoryRule {
        name: "units",
        allowed_categories: &["单位", "units"],
        required_fields: &["当前状态", "当前所在", "时间线状态"],
        required_headings: &["定位", "能力", "行为与表现", "写作约束"],
        required_heading_fields: &[
            ("定位", &["类型"]),
            ("能力", &["能力", "触发条件"]),
            ("行为与表现", &["外观识别点", "平时表现", "危机表现"]),
        ],
        id_prefix: "UNT-",
    },
    CategoryRule {
        name: "technology",
        allowed_categories: &["科技", "technology"],
        required_fields: &["技术等级", "当前适用区域", "时间线状态"],
        required_headings: &["定位", "核心设定", "社会影响", "写作约束"],
        required_heading_fields: &[
            ("定位", &["类型", "谁在掌握", "谁在使用"]),
            ("核心设定", &["功能", "依赖条件", "限制 / 风险"]),
            ("社会影响", &["高层用法", "底层用法", "价格 / 门槛"]),
        ],
        id_prefix: "TEC-",
    },
    CategoryRule {
        name: "economy",
        allowed_categories: &["经济", "economy"],
        required_fields: &["适用地点", "时间线状态"],
        required_headings: &["定位", "运行方式", "叙事作用", "写作约束"],
        required_heading_fields: &[
            ("定位", &["类型", "核心规则", "谁受益", "谁吃亏"]),
            ("运行方式", &["关键凭证 / 货币", "触发条件", "典型场景"]),
            ("叙事作用", &["能压出什么冲突", "容易显影在哪些场面"]),
        ],
        id_prefix: "ECO-",
    },
    CategoryRule {
        name: "organizations",
        allowed_categories: &["组织", "organizations"],
        required_fields: &["主要地点", "时间线状态"],
        required_headings: &["定位", "内部状态", "叙事作用", "写作约束"],
        required_heading_fields: &[
            ("定位", &["性质", "目标", "对外关系"]),
            ("内部状态", &["当前问题", "当前优势", "典型做事方式"]),
            ("叙事作用", &["能推动什么冲突"]),
        ],
        id_prefix: "ORG-",
    },
    CategoryRule {
        name: "items",
        allowed_categories: &["物品", "items"],
        required_fields: &["当前持有者", "当前位置", "数量状态"],
        required_headings: &["属性", "叙事用途", "写作约束"],
        required_heading_fields: &[
            ("属性", &["类型", "功能", "触发条件", "限制"]),
            ("叙事用途", &["当前作用", "潜在伏笔"]),
        ],
        id_prefix: "ITM-",
    },
    CategoryRule {
        name: "locations",
        allowed_categories: &["地点", "locations"],
        required_fields: &["所在区域", "时间线状态"],
        required_headings: &["定位", "场面特征", "设定", "写作约束"],
        required_heading_fields: &[
            ("定位", &["功能", "所属势力", "常驻人群"]),
            ("场面特征", &["视觉特征", "声音特征", "气味 / 触感"]),
            ("设定", &["特殊规则", "触发条件", "风险"]),
        ],
        id_prefix: "LOC-",
    },
    CategoryRule {
        name: "weather",
        allowed_categories: &["天气", "weather"],
        required_fields: &["主要地点", "时间线状态"],
        required_headings: &["定位", "影响", "叙事作用", "写作约束"],
        required_heading_fields: &[
            ("定位", &["类型", "持续条件", "触发来源"]),
            ("影响", &["对人", "对设备 / 建筑", "对战斗 / 交通"]),
            ("叙事作用", &["适合用来烘托什么", "不能替代什么真实剧情"]),
        ],
        id_prefix: "WTH-",
    },
    CategoryRule {
        name: "events",
        allowed_categories: &["事件", "events"],
        required_fields: &["发生地点", "时间线位置", "当前状态"],
        required_headings: &["触发", "过程", "解决与遗留", "写作约束"],
        required_heading_fields: &[
            ("触发", &["触发条件", "直接原因", "深层原因"]),
            ("过程", &["关键参与方", "表层结果", "隐性后果"]),
            ("解决与遗留", &["当前解决方法", "未解决问题"]),
        ],
        id_prefix: "EVT-",
    },
    CategoryRule {
        name: "meta",
        allowed_categories: &["补充", "meta"],
        required_fields: &["适用范围"],
        required_headings: &[],
        required_heading_fields: &[],
        id_prefix: "META-",
    },
];

fn normalize_heading(title: &str) -> String {
    title.replace("：", "").replace(":", "").trim().to_string()
}

type Heading = (u32, String, u32);

/// `parse_fields`：全文件扫描 `- 名：值` 字段；同名字段取末值
/// （保持首现位置、取末次值/行号）。
fn parse_fields(lines: &[&str]) -> Vec<(String, (u32, String))> {
    let mut fields: Vec<(String, (u32, String))> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if let Ok(Some(caps)) = FIELD_RE.captures(line) {
            let name = caps[1].trim().to_string();
            let value = caps[2].trim().to_string();
            let line_no = idx as u32 + 1;
            match fields.iter_mut().find(|(n, _)| *n == name) {
                Some(entry) => entry.1 = (line_no, value),
                None => fields.push((name, (line_no, value))),
            }
        }
    }
    fields
}

fn field_value<'a>(
    fields: &'a [(String, (u32, String))],
    name: &str,
) -> Option<(&'a u32, &'a String)> {
    fields.iter().find(|(n, _)| n == name).map(|entry| {
        let (_, pair) = entry;
        (&pair.0, &pair.1)
    })
}

fn parse_headings(lines: &[&str]) -> Vec<Heading> {
    let mut headings: Vec<Heading> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if let Ok(Some(caps)) = HEADING_RE.captures(line) {
            let level = caps[1].len() as u32;
            headings.push((idx as u32 + 1, normalize_heading(&caps[2]), level));
        }
    }
    headings
}

/// `collect_section_lines`：小节体为标题行之后到下一标题行之前（不含）。
fn collect_section_lines<'a>(
    lines: &'a [&'a str],
    headings: &[Heading],
) -> Vec<(String, Vec<&'a str>)> {
    let total = lines.len();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for (i, heading) in headings.iter().enumerate() {
        let next_line_no = headings.get(i + 1).map_or(total + 1, |h| h.0 as usize);
        let body: Vec<&str> = lines[heading.0 as usize..next_line_no - 1].to_vec();
        let title = heading.1.clone();
        match sections.iter_mut().find(|(n, _)| *n == title) {
            Some(entry) => entry.1 = body,
            None => sections.push((title, body)),
        }
    }
    sections
}

/// `parse_section_fields`：小节内的 `- 名：值` 字段；子项 `- x` 仅填充空值字段。
fn parse_section_fields(section_lines: &[&str]) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in section_lines {
        if let Ok(Some(caps)) = FIELD_RE.captures(line) {
            let name = caps[1].trim().to_string();
            let value = caps[2].trim().to_string();
            let idx = match values.iter().position(|(n, _)| *n == name) {
                Some(i) => i,
                None => {
                    values.push((name.clone(), String::new()));
                    values.len() - 1
                }
            };
            values[idx].1 = value;
            current = Some(idx);
            continue;
        }
        if let Some(i) = current {
            let stripped = line.trim();
            if stripped.starts_with("- ") && values[i].1.is_empty() {
                values[i].1 = stripped[2..].trim().to_string();
            }
        }
    }
    values
}

fn is_empty_value(value: &str) -> bool {
    let stripped = value.trim();
    if stripped.is_empty() {
        return true;
    }
    if matches!(stripped, "-" | "待补充" | "TBD") {
        return true;
    }
    stripped.contains("已确认 / 待确认")
}

/// `audit_card`：单张概念卡的全部告警（按固定检查顺序）。
pub fn audit_card(path: &Path) -> Result<Vec<Warning>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("无法读取概念卡文件 {}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let fields = parse_fields(&lines);
    let headings = parse_headings(&lines);
    let sections = collect_section_lines(&lines, &headings);
    let category = parent_name(path);
    let rule = CATEGORY_RULES.iter().find(|r| r.name == category);
    let Some(rule) = rule else {
        return Ok(vec![warn(
            0,
            "unknown_category",
            &format!("未知卡片分类目录：{category}"),
            &path.display().to_string(),
        )]);
    };

    let mut warnings: Vec<Warning> = Vec::new();
    for field_name in COMMON_FIELDS.iter().chain(rule.required_fields.iter()) {
        match field_value(&fields, field_name) {
            None => warnings.push(warn(
                0,
                "missing_field",
                &format!("缺少字段：{field_name}"),
                "",
            )),
            Some((line_no, value)) => {
                if is_empty_value(value) {
                    warnings.push(warn(
                        *line_no,
                        "empty_field",
                        &format!("字段为空：{field_name}"),
                        field_name,
                    ));
                }
            }
        }
    }

    if let Some((line_no, value)) = field_value(&fields, "所属分类") {
        if !rule.allowed_categories.iter().any(|c| *c == value) {
            warnings.push(warn(
                *line_no,
                "category_mismatch",
                &format!("所属分类与目录不匹配：{value}"),
                value,
            ));
        }
    }

    if let Some((line_no, value)) = field_value(&fields, "卡片 ID") {
        if !is_empty_value(value) && !value.starts_with(rule.id_prefix) {
            warnings.push(warn(
                *line_no,
                "bad_id_prefix",
                &format!("卡片 ID 前缀应为 {}", rule.id_prefix),
                value,
            ));
        }
    }

    let heading_names: Vec<&str> = headings.iter().map(|h| h.1.as_str()).collect();
    for heading_name in rule.required_headings {
        if !heading_names.iter().any(|t| t == heading_name) {
            warnings.push(warn(
                0,
                "missing_heading",
                &format!("缺少段落：{heading_name}"),
                "",
            ));
        }
    }

    for (heading_name, required_fields) in rule.required_heading_fields {
        let section_values = match sections.iter().find(|(n, _)| n == heading_name) {
            Some((_, lines)) => parse_section_fields(lines),
            None => Vec::new(),
        };
        for field_name in *required_fields {
            match section_values.iter().find(|(n, _)| n == field_name) {
                None => {
                    warnings.push(warn(
                        0,
                        "missing_section_field",
                        &format!("{heading_name} 缺少字段：{field_name}"),
                        heading_name,
                    ));
                }
                Some((_, value)) => {
                    if is_empty_value(value) {
                        warnings.push(warn(
                            0,
                            "empty_section_field",
                            &format!("{heading_name} 字段为空：{field_name}"),
                            field_name,
                        ));
                    }
                }
            }
        }
    }

    if let Some((line_no, value)) = field_value(&fields, "关联卡片") {
        if is_empty_value(value) {
            warnings.push(warn(
                *line_no,
                "empty_relation",
                "关联卡片为空，后续文件很难追依赖",
                "关联卡片",
            ));
        }
    }

    Ok(warnings)
}

/// 父目录名；裸文件名（无父目录）时为 "."。
fn parent_name(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string())
}

/// `format_report`：markdown 报告（输出逐字节稳定）。
pub fn format_report(path: &Path, warnings: &[Warning]) -> String {
    let status = if warnings.is_empty() { "OK" } else { "WARN" };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mut lines = vec![
        format!("# {name}"),
        String::new(),
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

/// `iter_targets`：目录递归找 `*.md`（按路径排序，`_templates` 目录默认排除）；
/// 文件直接入列。排除 readme.md 与（未开启 include_templates 时）_templates 下的文件。
pub fn iter_targets(raw_paths: &[PathBuf], include_templates: bool) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = Vec::new();
    for raw in raw_paths {
        if raw.is_dir() {
            let mut found: Vec<PathBuf> = Vec::new();
            walk_md_dir(raw, &mut found);
            found.sort();
            for path in found {
                if !include_templates && in_templates_dir(&path) {
                    continue;
                }
                let is_readme = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase() == "readme.md");
                if is_readme {
                    continue;
                }
                targets.push(path);
            }
        } else if raw.is_file() {
            if !include_templates && in_templates_dir(raw) {
                continue;
            }
            let is_readme = raw
                .file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase() == "readme.md");
            if is_readme {
                continue;
            }
            targets.push(raw.clone());
        }
    }
    targets
}

/// 路径部件（Unix 分隔）是否含 `_templates`。
fn in_templates_dir(path: &Path) -> bool {
    path.to_string_lossy()
        .split('/')
        .any(|part| part == "_templates")
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

/// `main` 全流程：解析输入 → 逐卡审计 → stdout 报告。
/// 有警告时返回退出码 1（概念卡无 `--fail-on-warn` 开关，任何警告都返回 1）。
pub fn run(paths: &[PathBuf], include_templates: bool) -> Result<i32> {
    let targets = iter_targets(paths, include_templates);
    if targets.is_empty() {
        anyhow::bail!("No concept cards found.");
    }
    let mut warned = false;
    for path in &targets {
        let warnings = audit_card(path)?;
        println!("{}", format_report(path, &warnings));
        println!();
        if !warnings.is_empty() {
            warned = true;
        }
    }
    Ok(if warned { 1 } else { 0 })
}
