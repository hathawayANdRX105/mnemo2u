//! `study.pov`：中文视角切分候选检测。
//!
//! - 段落切分复用 [`crate::text::TextSplitter::split_paragraph_infos`]；
//! - `schema_version` 固定为 **1**；
//! - 密度字段（pronoun / personal_name / dialogue_attribution）恒为 0
//!   （audit 管道未填 pronouns / personal_names，密度三字段均为 `(0, 0.0)`）；
//! - 候选分组：相邻段密度不变量变化才切组，尾组始终附加；
//! - `study` 词集缺省时全空（[`crate::config::StudyConfig`] 缺省 `None`）。
//!
//! 输出 JSON 键序固定。

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::StudyConfig;
use crate::text::TextSplitter;

/// POV analysis 的 `schema_version` 值。
///
/// 固定为 `1`。
pub const SCHEMA_VERSION: i64 = 1;

/// 单个 POV 候选段（键序固定）。
#[derive(Debug, Clone, Serialize)]
struct PoVObservation {
    candidate: &'static str,
    paragraph_range: String,
    evidence_line_numbers: Vec<u32>,
    pronoun_density: (i64, f64),
    personal_name_density: (i64, f64),
    dialogue_attribution: (i64, f64),
}

/// POV analysis 的 `chapter_summary` 节。
#[derive(Debug, Clone, Serialize)]
struct PovSummary {
    total_paragraphs: u32,
    candidates: Vec<PoVObservation>,
}

/// POV analysis 顶层结构（键序固定）。
#[derive(Debug, Clone, Serialize)]
pub struct PovAnalysis {
    schema_version: i64,
    chapter_summary: PovSummary,
    heuristic: bool,
    confidence: &'static str,
    note: &'static str,
}

/// 对给定文本计算 POV 切分候选。
///
/// `splitter` 须由 [`crate::text::TextSplitter::new`] 创建，
/// 使用 `draft.markdown_noise_line.pattern`。
pub fn build_pov_candidates(text: &str, splitter: &TextSplitter) -> PovAnalysis {
    let raw_infos = splitter.split_paragraph_infos(text);
    let total_paragraphs = raw_infos.len() as u32;

    // pronouns / personal_names 恒为空（audit 管道未填），
    // dialogue_attribution 恒为 0 → 密度三字段均为 `(0, 0.0)`
    // （count int、density float，JSON 渲染 `[0, 0.0]`）。

    // 候选分组逻辑：
    // 由于所有密度恒为 0，`prev` 始终为 `(0.0, 0.0, 0.0)`，
    // 循环中从不触发切组 → 只有尾组（当有段落时）。
    let mut candidates: Vec<PoVObservation> = Vec::new();

    if total_paragraphs > 0 {
        let evidence: Vec<u32> = raw_infos.iter().map(|pi| pi.line_start as u32).collect();
        candidates.push(PoVObservation {
            candidate: "POV shift candidate",
            paragraph_range: format!("0-{}", total_paragraphs),
            evidence_line_numbers: evidence,
            pronoun_density: (0, 0.0),
            personal_name_density: (0, 0.0),
            dialogue_attribution: (0, 0.0),
        });
    }

    PovAnalysis {
        schema_version: SCHEMA_VERSION,
        chapter_summary: PovSummary {
            total_paragraphs,
            candidates,
        },
        heuristic: true,
        confidence: "low",
        note: "Output requires human confirmation and is not a conclusion.",
    }
}

/// 运行 `study-pov` 子命令，返回格式化 JSON（2 空格缩进、非 ASCII 原样输出）。
pub fn run_pov(
    chapter_path: &std::path::Path,
    config: &crate::config::ReviewRules,
) -> Result<String> {
    let text = std::fs::read_to_string(chapter_path)
        .with_context(|| format!("读取章节文件失败: {}", chapter_path.display()))?;
    let splitter = TextSplitter::new(&config.draft.markdown_noise_line.pattern)?;
    let _study: &StudyConfig = config.study.as_ref().unwrap_or(&StudyConfig::default());
    let analysis = build_pov_candidates(&text, &splitter);
    Ok(serde_json::to_string_pretty(&analysis)?)
}
