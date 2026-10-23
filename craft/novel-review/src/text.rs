//! 文本基础工具：句子/段落拆分、字数统计、引用比例。
//!
//! 拆分/统计函数：`split_sentences`、
//! `split_sentence_infos`、`split_paragraph_infos`、`prose_char_count`、
//! `quote_ratio`。句子拆分按行执行，并跳过 Markdown 噪音行。

use anyhow::{Context, Result};
use regex::Regex;
use std::sync::LazyLock;

/// 句子切分正则（`SENTENCE_SPLIT`）。
pub const SENTENCE_SPLIT_PATTERN: &str = r"[。！？!?]+|\n+";
/// 句子块正则（`SENTENCE_CHUNK`）。
pub const SENTENCE_CHUNK_PATTERN: &str = r"[^。！？!?\n]+(?:[。！？!?]+|$)";
/// 对白行判定正则（`QUOTE_LINE`）。
pub const QUOTE_LINE_PATTERN: &str = r#"^\s*[“"【].*"#;
/// 句首可剥离的标点集合（`LEADING_PUNCT`）。
pub const LEADING_PUNCT: &str = "“”\"'【】《》〈〉（）()[]「」『』，,：:；;、 ";

/// 句子信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceInfo {
    /// 句子序号，从 1 开始。
    pub index: usize,
    /// 所在行号，从 1 开始。
    pub line_no: usize,
    /// 句子文本（已去掉首尾句末标点）。
    pub text: String,
    /// 可数字符数（中文/字母/数字）。
    pub chars: usize,
}

/// 段落信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParagraphInfo {
    /// 段落序号，从 1 开始。
    pub index: usize,
    /// 起始行号，从 1 开始。
    pub line_start: usize,
    /// 结束行号，从 1 开始。
    pub line_end: usize,
    /// 段落文本。
    pub text: String,
    /// 可数字符数。
    pub chars: usize,
    /// 是否判定为对白段。
    pub is_dialogue: bool,
}

/// 句子/段落拆分器，持有编译好的正则。
pub struct TextSplitter {
    sentence_split: Regex,
    sentence_chunk: Regex,
    markdown_noise: Regex,
}

impl TextSplitter {
    /// 创建拆分器。
    ///
    /// `markdown_noise_pattern` 来自 `draft.markdown_noise_line.pattern`。
    /// 编译任一正则失败时返回错误。
    pub fn new(markdown_noise_pattern: &str) -> Result<Self> {
        Ok(Self {
            sentence_split: Regex::new(SENTENCE_SPLIT_PATTERN).context("编译句子拆分正则失败")?,
            sentence_chunk: Regex::new(SENTENCE_CHUNK_PATTERN).context("编译句子块正则失败")?,
            markdown_noise: Regex::new(markdown_noise_pattern)
                .context("编译 Markdown 噪音正则失败")?,
        })
    }

    /// 按句末标点/换行切分句子，返回去空白后的句子列表。
    pub fn split_sentences(&self, text: &str) -> Vec<String> {
        self.sentence_split
            .split(text)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 逐行切分句子并生成 `SentenceInfo`，跳过空行与 Markdown 噪音行。
    pub fn split_sentence_infos(&self, text: &str) -> Vec<SentenceInfo> {
        let mut infos = Vec::new();
        for (idx, line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let stripped = line.trim();
            if stripped.is_empty() || self.markdown_noise.is_match(stripped) {
                continue;
            }
            for matched in self.sentence_chunk.find_iter(line) {
                let sentence = matched
                    .as_str()
                    .trim()
                    .trim_matches(['。', '！', '？', '!', '?']);
                if sentence.is_empty() {
                    continue;
                }
                let chars = prose_char_count(sentence);
                if chars == 0 {
                    continue;
                }
                infos.push(SentenceInfo {
                    index: infos.len() + 1,
                    line_no,
                    text: sentence.to_string(),
                    chars,
                });
            }
        }
        infos
    }

    /// 按空行分组合法段落并生成 `ParagraphInfo`，识别对白段。
    pub fn split_paragraph_infos(&self, text: &str) -> Vec<ParagraphInfo> {
        let mut infos = Vec::new();
        let lines: Vec<&str> = text.lines().collect();
        let mut chunk_lines: Vec<String> = Vec::new();
        let mut start_line = 1usize;

        for (idx, raw_line) in lines.iter().enumerate() {
            let line_no = idx + 1;
            if !raw_line.trim().is_empty() {
                if chunk_lines.is_empty() {
                    start_line = line_no;
                }
                chunk_lines.push(raw_line.trim_end().to_string());
                continue;
            }
            if !chunk_lines.is_empty() {
                if let Some(info) =
                    flush_paragraph(&chunk_lines, start_line, line_no - 1, infos.len())
                {
                    infos.push(info);
                }
                chunk_lines.clear();
            }
        }
        if !chunk_lines.is_empty() {
            if let Some(info) = flush_paragraph(&chunk_lines, start_line, lines.len(), infos.len())
            {
                infos.push(info);
            }
        }
        infos
    }
}

/// 把一组非空行组装为 `ParagraphInfo`；文本为空时返回 `None`。
fn flush_paragraph(
    chunk_lines: &[String],
    start_line: usize,
    line_end: usize,
    index: usize,
) -> Option<ParagraphInfo> {
    let paragraph_text = chunk_lines.join("\n");
    let paragraph_text = paragraph_text.trim();
    if paragraph_text.is_empty() {
        return None;
    }
    let paragraph_lines: Vec<&str> = paragraph_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let is_dialogue = !paragraph_lines.is_empty()
        && (paragraph_lines.iter().all(|line| is_quote_line(line))
            || (paragraph_lines.len() == 1
                && quote_ratio(paragraph_lines[0]) > 0.02
                && paragraph_lines[0].contains('“')));
    Some(ParagraphInfo {
        index: index + 1,
        line_start: start_line,
        line_end,
        text: paragraph_text.to_string(),
        chars: prose_char_count(paragraph_text),
        is_dialogue,
    })
}

/// 统计可数字符（中文/字母/数字），对应 `prose_char_count`。
pub fn prose_char_count(text: &str) -> usize {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .count()
}

/// 引用符号占全文字符比例，对应 `quote_ratio`。
pub fn quote_ratio(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let quote_chars = text
        .chars()
        .filter(|c| matches!(c, '“' | '”' | '"' | '【' | '】'))
        .count();
    quote_chars as f64 / text.chars().count().max(1) as f64
}

/// 是否对白行：行首为引号/【，对应 `QUOTE_LINE.match`。
static QUOTE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(QUOTE_LINE_PATTERN).expect("QUOTE_LINE 正则应可编译"));

fn is_quote_line(line: &str) -> bool {
    QUOTE_LINE_RE.is_match(line)
}
