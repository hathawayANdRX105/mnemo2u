//! 大纲（arc/story/chapter）plan 文件审查：收集标题、小节与标签块，输出规则告警与 markdown 报告。
//!
//! 语义契约：
//! - 标题解析、小节收集、标签块解析（同名的「同键覆盖」语义用「按位置替换」模拟）；
//! - 排序/取最小值/平手处理按契约执行（本项目 plan 节无 float 指标，
//!   仅排序与词数统计）；
//! - 正则统一 `fancy_regex`；`ignore_case` 规则在 pattern 前缀 `(?i)`；
//! - 非重叠计数（`str::matches`）；码点计数（`chars().count()`）；
//! - 报告渲染（markdown）为逐字节稳定输出。

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use fancy_regex::Regex;
use serde::Serialize;

use crate::config::{self, PlanConfig, PlanRegexRule};
use crate::input;

/// 大纲审查警告（`Warning` 结构体，字段序固定）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    pub line_no: u32,
    pub kind: String,
    pub message: String,
    pub snippet: String,
}

/// 构造一条警告（字段序按结构体定义）。
fn warn(line_no: u32, kind: &str, message: &str, snippet: &str) -> Warning {
    Warning {
        line_no,
        kind: kind.to_string(),
        message: message.to_string(),
        snippet: snippet.to_string(),
    }
}

/// 报告输出格式（对应 `--format` 参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    Markdown,
}

// 模块级正则（HEADING 用 `^` 锚定，取首个命中即等价于 match 语义）。
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"^(#{1,6})\s+(.+?)\s*$").unwrap());
static LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"^\s*-\s*([^：:]+)[：:]\s*(.*)$").unwrap());
static LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"^\s*(?:-\s+|\d+\.\s+|\*\s+)(.+)$").unwrap());
static SCENE_TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"^Scene\s+\d+").unwrap());

fn re_search(re: &Regex, text: &str) -> bool {
    matches!(re.find(text), Ok(Some(_)))
}

fn re_find_all(re: &Regex, text: &str) -> usize {
    re.find_iter(text).filter(|m| m.is_ok()).count()
}

/// 码点数。
fn cpl(s: &str) -> u32 {
    s.chars().count() as u32
}

/// 字符串切片：取前 n 个码点。
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn contains_any(text: &str, terms: &[String]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

/// 标题结构：(line_no, 规范化标题, 级别)。
pub type Heading = (u32, String, u32);

/// 小节行：(line_no, 原始行)。
pub type BodyLine = (u32, String);

/// 小节容器：键序=首次插入序、值=最后一次赋值（同键覆盖语义）。
pub struct Sections {
    names: Vec<String>,
    bodies: Vec<Vec<BodyLine>>,
}

impl Sections {
    fn insert(&mut self, name: &str, body: Vec<BodyLine>) {
        match self.names.iter().position(|n| n == name) {
            Some(i) => self.bodies[i] = body,
            None => {
                self.names.push(name.to_string());
                self.bodies.push(body);
            }
        }
    }

    /// `sections.get(name, [])` 语义：不存在时返回空。
    fn body(&self, name: &str) -> &[BodyLine] {
        self.names
            .iter()
            .position(|n| n == name)
            .map_or(&[], |i| &self.bodies[i])
    }

    /// `find_section`：按候选顺序取第一个存在的小节名。
    fn find<'a>(&self, choices: &[&'a str]) -> Option<&'a str> {
        choices
            .iter()
            .copied()
            .find(|choice| self.names.iter().any(|n| n == *choice))
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &[BodyLine])> {
        self.names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), &self.bodies[i][..]))
    }
}

/// 标签块容器：`find_label_blocks` 语义（键序=首现序，同键追加）。
struct LabelBlocks {
    labels: Vec<String>,
    blocks: Vec<Vec<BodyLine>>,
}

impl LabelBlocks {
    fn get(&self, name: &str) -> &[BodyLine] {
        self.labels
            .iter()
            .position(|l| l == name)
            .map_or(&[], |i| &self.blocks[i])
    }

    fn has(&self, name: &str) -> bool {
        self.labels.iter().any(|l| l == name)
    }
}

/// 已编译的 10 条 plan 正则。
struct PlanRegs {
    style_leak: Regex,
    abstract_lookpoint: Regex,
    answer_leak: Regex,
    scene_leak: Regex,
    prose_leak: Regex,
    judgement: Regex,
    hookish: Regex,
    foreshadow_id: Regex,
    rhythm_style_leak: Regex,
    story_prose_leak: Regex,
}

/// 大纲审查引擎：持有 plan 节配置 + 编译后的正则。
pub struct PlanEngine {
    cfg: PlanConfig,
    regs: PlanRegs,
    paths: crate::config::PathsConfig,
}

fn compile_rule(rules: &HashMap<String, PlanRegexRule>, name: &str) -> Result<Regex> {
    let rule = rules
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Missing review rules section: plan.regex.{name}"))?;
    let pattern = if rule.ignore_case {
        format!("(?i){}", rule.pattern)
    } else {
        rule.pattern.clone()
    };
    Regex::new(&pattern).with_context(|| format!("plan 正则 {name} 编译失败: {pattern}"))
}

impl PlanEngine {
    /// 由 `plan` 节与 `paths` 节配置构建引擎；缺节/正则非法时返回错误。
    pub fn new(cfg: &PlanConfig, paths: &crate::config::PathsConfig) -> Result<Self> {
        let regs = PlanRegs {
            style_leak: compile_rule(&cfg.regex, "style_leak")?,
            abstract_lookpoint: compile_rule(&cfg.regex, "abstract_lookpoint")?,
            answer_leak: compile_rule(&cfg.regex, "answer_leak")?,
            scene_leak: compile_rule(&cfg.regex, "scene_leak")?,
            prose_leak: compile_rule(&cfg.regex, "prose_leak")?,
            judgement: compile_rule(&cfg.regex, "judgement")?,
            hookish: compile_rule(&cfg.regex, "hookish")?,
            foreshadow_id: compile_rule(&cfg.regex, "foreshadow_id")?,
            rhythm_style_leak: compile_rule(&cfg.regex, "rhythm_style_leak")?,
            story_prose_leak: compile_rule(&cfg.regex, "story_prose_leak")?,
        };
        Ok(Self {
            cfg: cfg.clone(),
            regs,
            paths: paths.clone(),
        })
    }

    /// `paths` 节：小说目录布局。
    pub fn paths(&self) -> &crate::config::PathsConfig {
        &self.paths
    }

    /// `CHAPTER_FUNCTION_RULES`（`plan.function_rules.chapter`）。
    pub fn chapter_function_rules(&self) -> &HashMap<String, Vec<String>> {
        &self.cfg.function_rules.chapter
    }

    /// `ENDING_FUNCTION_RULES`（`plan.function_rules.ending`）。
    pub fn ending_function_rules(&self) -> &HashMap<String, Vec<String>> {
        &self.cfg.function_rules.ending
    }

    /// `CHAPTER_ENDING_GROUP`（`plan.chapter_ending_group`）。
    pub fn chapter_ending_group(&self) -> &[String] {
        &self.cfg.chapter_ending_group
    }
}

mod parse;
mod report;

pub use parse::{
    audit_file, bullet_lines, collect_section_lines, detect_function_label, find_section,
    parse_headings, section_text,
};
pub use report::{format_report, iter_targets, run};
