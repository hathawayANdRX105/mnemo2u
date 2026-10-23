//! `audit.draft` 完整分析：`analyze_text` 返回**全部**顶层节。
//!
//! - 9 个规则指标节（tokens/patterns/phrases/modifiers/punctuation/
//!   punctuation_combos/custom_templates/tracked_terms/tracked_term_categories）
//!   复用 [`crate::rules`] 既有实现；
//! - 其余结构分析（summary、warned、句长/对白/场面/语料/疲劳/提醒等）在本模块实现，
//!   语义按固定契约（含平手按首现序、
//!   `round(x, n)` 半偶舍入、码点计数、`min/max` 平手取首等语义坑）；
//! - text/markdown 渲染：`format_text_report` / `format_markdown_report` / `_render_report` 输出逐字节稳定。
//!
//! JSON 键名遵循固定契约；浮点值经 [`crate::rules::round2`]（2 位）
//! 与 [`round4f`]（4 位）舍入（对二进制精确值半偶舍入）。
//!
//! 子模块：`output`（JSON 输出结构）、`collect`（输入发现/语料清洗/跟踪词窗口/把字操作）、
//! `sentence`（句长/句首/ngram/语料画像）、`dialogue`（对白检测/情绪/角色声音）、
//! `scene`（段落功能/场景地图/语气/战斗/视角）、`fatigue`（疲劳窗口/风格疲劳）、
//! `reminders`（审查提醒）、`analyze`/`assemble`（`analyze_text` 前后装配）、
//! `report_text`/`report_markdown`（报告渲染与 CLI 入口）。

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fancy_regex::Regex;
use serde::Serialize;

use crate::config::ReviewRules;
use crate::config::{TemplateRule, TrackedTerm};
use crate::input::{resolve_inputs, write_json_line, write_text};
use crate::rules::{
    build_custom_template_metrics, build_rule_metrics, build_template_bank,
    build_tracked_term_metrics, density, round2, CompiledRule, CustomTemplateMetric, Hit,
    RegexMetric,
};
use crate::text::{prose_char_count, quote_ratio, TextSplitter};

// ---------------------------------------------------------------------------
// 常量与共享正则（模块级编译一次）

/// 重叠词（AA/BB/AABB）正则，对齐 `collect_aa_bb_patterns` 内联正则。
const AA_BB_RE: &str = r"(?:一([\u{4e00}-\u{9fff}])\1|([\u{4e00}-\u{9fff}])\2([\u{4e00}-\u{9fff}])\3|([\u{4e00}-\u{9fff}]{2})\4)";
/// 把字操作片段正则（`FATIGUE_WINDOW_BA_RE`）。
const BA_RE: &str = r"把[^，。！？!?]{1,24}";
/// 分句切分（`CLAUSE_SPLIT`）。
const CLAUSE_SPLIT_RE: &str = r"[，；：]";
/// ngram 清洗：只保留 CJK 与 ASCII 字母（`re.sub(r"[^\u4e00-\u9fffA-Za-z]", "", text)`）。
const NGRAM_KEEP_RE: &str = r"[^一-龥A-Za-z]";
/// 语料正文清洗：列表符前缀（`re.sub(r"^[-*]\s*", "", s)`）。
const CORPUS_BULLET_RE: &str = r"^[-*]\s*";
/// 语料正文清洗：反引号包裹（`re.sub(r"`([^`]+)`", r"\1", s)`）。
const CORPUS_BACKTICK_RE: &str = r"`([^`]+)`";
/// 语料正文清洗：粗体包裹（`re.sub(r"\*\*([^*]+)\*\*", r"\1", s)`）。
const CORPUS_BOLD_RE: &str = r"\*\*([^*]+)\*\*";
/// 语料清洗的分隔符整行（`re.fullmatch(r"[-:| ]+", s)`）。
const CORPUS_TABLE_LINE_RE: &str = r"[-:| ]+";
/// 纯 ASCII 数字字母（`re.fullmatch(r"[A-Za-z0-9]+", s)`）。
const ALPHA_NUMERIC_RE: &str = r"[A-Za-z0-9]+";
/// ngram 字母判定（`phrase.isascii() and phrase.isalpha()`）。
const ASCII_ALPHA_RE: &str = r"[A-Za-z]+";

/// 计数语义：计数 + 首现序；`most_common` 平手按首现序。
#[derive(Debug, Clone, Default)]
pub struct Counter {
    /// 按 key 首现序排列。
    entries: Vec<(String, usize)>,
    index: HashMap<String, usize>,
}

/// 计数器 JSON 对象：按首现序序列化为 JSON object。
#[derive(Debug, Clone, Default)]
pub struct CountMap(Vec<(String, usize)>);

impl CountMap {
    #[must_use]
    pub fn new(entries: Vec<(String, usize)>) -> Self {
        Self(entries)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, (String, usize)> {
        self.0.iter()
    }
}

impl Serialize for CountMap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, count) in &self.0 {
            map.serialize_entry(key, count)?;
        }
        map.end()
    }
}

impl Counter {
    /// 新增一次计数；返回该 key 的累计计数。
    pub fn add(&mut self, key: &str) -> usize {
        match self.index.get(key) {
            Some(&i) => {
                self.entries[i].1 += 1;
                self.entries[i].1
            }
            None => {
                self.index.insert(key.to_string(), self.entries.len());
                self.entries.push((key.to_string(), 1));
                1
            }
        }
    }

    /// 新增 `n` 次计数；返回该 key 的累计计数。
    pub fn add_n(&mut self, key: &str, n: usize) -> usize {
        match self.index.get(key) {
            Some(&i) => {
                self.entries[i].1 += n;
                self.entries[i].1
            }
            None => {
                self.index.insert(key.to_string(), self.entries.len());
                self.entries.push((key.to_string(), n));
                n
            }
        }
    }

    /// 取累计计数（未出现过为 0）。
    pub fn get(&self, key: &str) -> usize {
        self.index.get(key).map_or(0, |&i| self.entries[i].1)
    }

    /// 按首现序遍历 (key, count)。
    pub fn entries(&self) -> &[(String, usize)] {
        &self.entries
    }

    /// 是否出现过任何 key。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 总计数。
    pub fn total(&self) -> usize {
        self.entries.iter().map(|e| e.1).sum()
    }

    /// `Counter.most_common()`：计数降序；平手保持首现序（稳定排序）。
    pub fn most_common_all(&self) -> Vec<(String, usize)> {
        let mut v = self.entries.clone();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v
    }

    /// `Counter.most_common(n)`。
    pub fn most_common(&self, n: usize) -> Vec<(String, usize)> {
        let mut v = self.entries.clone();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v.truncate(n);
        v
    }
}

// ---------------------------------------------------------------------------
// 数值/码点语义辅助

/// `round(x, n)`：对二进制精确值做半偶舍入（对齐既有 [`round2`]，推广到 n 位）。
#[must_use]
pub fn round_nd(x: f64, n: u32) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    let sign = x < 0.0;
    let abs = x.abs();
    let bits = abs.to_bits();
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let scale: u128 = 10u128.pow(n);
    let scaled = (mantissa as u128) * scale; // x * 10^n = scaled * 2^exponent
    let nearest: u128 = if exponent >= 0 {
        scaled << exponent
    } else {
        let shift = (-exponent) as u32;
        if shift >= 128 {
            0
        } else {
            let low = scaled & ((1u128 << shift) - 1);
            let half = 1u128 << (shift - 1);
            let mut quotient = scaled >> shift;
            if low > half || (low == half && quotient & 1 == 1) {
                quotient += 1;
            }
            quotient
        }
    };
    let v = (nearest as f64) / scale as f64;
    if sign {
        -v
    } else {
        v
    }
}

/// `round(x, 4)`。
#[must_use]
pub fn round4f(x: f64) -> f64 {
    round_nd(x, 4)
}

/// `round(x)`（0 位）：半偶取整。
#[must_use]
pub fn bankers_round_int(x: f64) -> i64 {
    if x == 0.0 {
        return 0;
    }
    let sign = x < 0.0;
    let abs = x.abs();
    let bits = abs.to_bits();
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let nearest: u128 = if exponent >= 0 {
        mantissa as u128 >> exponent
    } else {
        let shift = (-exponent) as u32;
        if shift >= 128 {
            0
        } else {
            let scaled = mantissa as u128;
            let low = scaled & ((1u128 << shift) - 1);
            let half = 1u128 << (shift - 1);
            let mut q = scaled >> shift;
            if low > half || (low == half && q & 1 == 1) {
                q += 1;
            }
            q
        }
    };
    if sign {
        -(nearest as i64)
    } else {
        nearest as i64
    }
}

/// 浮点数值展示：shortest roundtrip，整数值带 `.0`
/// （12.0 显示为 "12.0"；Rust 默认 `{}` 格式化为 "12"）。
#[must_use]
pub fn float_repr(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// 取字符串末尾 n 个码点。
fn tail_chars(text: &str, n: usize) -> &str {
    let total = text.chars().count();
    if total <= n {
        return text;
    }
    let skip = total - n;
    let start = text
        .char_indices()
        .nth(skip)
        .map(|(b, _)| b)
        .unwrap_or(text.len());
    &text[start..]
}

/// 字符串码点数。
fn code_len(s: &str) -> usize {
    s.chars().count()
}

/// 取字符串前 n 个码点。
fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 首尾/首部剥离指定字符集。
fn strip_chars<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_matches(|c: char| set.chars().any(|x| x == c))
}
fn lstrip_chars<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_start_matches(|c: char| set.chars().any(|x| x == c))
}

/// `LEADING_PUNCT` 词表常量。
const LEADING_PUNCT: &str = "“”\"'【】《》〈〉（）()[]「」『』，,：:；;、 ";

// ---------------------------------------------------------------------------
// 分析上下文（编译一次，逐篇复用）

/// 一次分析会话所需的配置与编译正则（编译一次，逐篇复用）。
pub struct DraftContext {
    rules: ReviewRules,
    splitter: TextSplitter,
    token_rules: Vec<CompiledRule>,
    pattern_rules: Vec<CompiledRule>,
    phrase_rules: Vec<CompiledRule>,
    modifier_rules: Vec<CompiledRule>,
    punctuation_rules: Vec<CompiledRule>,
    combo_rules: Vec<CompiledRule>,
    speaker_patterns: Vec<Regex>,
    speaker_line_patterns: Vec<Regex>,
    connective_patterns: Vec<(String, Regex)>,
    aa_bb_regex: Regex,
    ba_regex: Regex,
    clause_split_regex: Regex,
    ngram_keep_regex: Regex,
    corpus_bullet_regex: Regex,
    corpus_backtick_regex: Regex,
    corpus_bold_regex: Regex,
    corpus_table_line_regex: Regex,
    corpus_markdown_noise_regex: Regex,
    alpha_numeric_regex: Regex,
    ascii_alpha_regex: Regex,
}

fn compile_rules(list: &[crate::config::RegexRule]) -> Result<Vec<CompiledRule>> {
    list.iter().map(CompiledRule::compile).collect()
}

impl DraftContext {
    /// 构建上下文：编译全部规则与共享正则（正则非法时报错）。
    pub fn new(rules: ReviewRules) -> Result<Self> {
        let draft = &rules.draft;
        let splitter = TextSplitter::new(&draft.markdown_noise_line.pattern)?;
        let speaker_patterns: Vec<Regex> = draft
            .speaker
            .patterns
            .iter()
            .map(|p| Regex::new(p).with_context(|| format!("speaker pattern {p:?} 无法编译")))
            .collect::<Result<Vec<_>>>()?;
        let speaker_line_patterns: Vec<Regex> = draft
            .speaker
            .line_patterns
            .iter()
            .map(|p| Regex::new(p).with_context(|| format!("speaker line pattern {p:?} 无法编译")))
            .collect::<Result<Vec<_>>>()?;
        let connective_patterns: Vec<(String, Regex)> = draft
            .connective_sentence_patterns
            .iter()
            .map(|c| {
                let regex = Regex::new(&c.pattern)
                    .with_context(|| format!("连接词句首 {:?} 无法编译", c.label))?;
                Ok((c.label.clone(), regex))
            })
            .collect::<Result<Vec<_>>>()?;
        let token_rules = compile_rules(&draft.regex_rules.tokens)?;
        let pattern_rules = compile_rules(&draft.regex_rules.patterns)?;
        let phrase_rules = compile_rules(&draft.regex_rules.phrases)?;
        let modifier_rules = compile_rules(&draft.regex_rules.modifiers)?;
        let punctuation_rules = compile_rules(&draft.regex_rules.punctuation)?;
        let combo_rules = compile_rules(&draft.regex_rules.punctuation_combos)?;
        let noise_pattern = draft.markdown_noise_line.pattern.clone();
        Ok(Self {
            rules,
            splitter,
            token_rules,
            pattern_rules,
            phrase_rules,
            modifier_rules,
            punctuation_rules,
            combo_rules,
            speaker_patterns,
            speaker_line_patterns,
            connective_patterns,
            aa_bb_regex: Regex::new(AA_BB_RE).expect("AA/BB 正则应可编译"),
            ba_regex: Regex::new(BA_RE).expect("把字句正则应可编译"),
            clause_split_regex: Regex::new(CLAUSE_SPLIT_RE).expect("分句切分正则应可编译"),
            ngram_keep_regex: Regex::new(NGRAM_KEEP_RE).expect("ngram 清洗正则应可编译"),
            corpus_bullet_regex: Regex::new(CORPUS_BULLET_RE).expect("列表符正则应可编译"),
            corpus_backtick_regex: Regex::new(CORPUS_BACKTICK_RE).expect("反引号正则应可编译"),
            corpus_bold_regex: Regex::new(CORPUS_BOLD_RE).expect("粗体正则应可编译"),
            corpus_table_line_regex: Regex::new(CORPUS_TABLE_LINE_RE)
                .expect("表格分隔行正则应可编译"),
            corpus_markdown_noise_regex: Regex::new(&noise_pattern)
                .expect("markdown 噪音行正则应可编译"),
            alpha_numeric_regex: Regex::new(ALPHA_NUMERIC_RE).expect("ASCII 正则应可编译"),
            ascii_alpha_regex: Regex::new(ASCII_ALPHA_RE).expect("ASCII 字母正则应可编译"),
        })
    }

    /// 草稿规则节（模板库/跟踪词等；stats 子命令需要）。
    pub fn draft_rules(&self) -> &crate::config::DraftConfig {
        &self.rules.draft
    }

    /// 阈值配置。
    pub fn thresholds(&self) -> &crate::config::DraftThresholds {
        &self.rules.draft.thresholds
    }

    /// 小说目录布局（`paths` 节）。
    pub fn paths(&self) -> &crate::config::PathsConfig {
        &self.rules.paths
    }

    /// 语料学习窗口配置。
    pub fn learned_window(&self) -> &crate::config::LearnedTermWindow {
        &self.rules.draft.learned_term_window
    }

    /// 词库。
    pub fn lexicon(&self) -> &crate::config::Lexicon {
        &self.rules.draft.lexicon
    }

    /// 拆分器。
    pub fn splitter(&self) -> &TextSplitter {
        &self.splitter
    }

    /// 语料自动发现的相对目录：概念卡 + 三类大纲 + 正文。
    ///
    /// 目录名来自 `paths` 节，不另有一份配置——两份配置会各自漂移。
    pub fn default_corpus_parts(&self) -> Vec<Vec<String>> {
        let paths = self.paths();
        let mut parts = vec![paths
            .cards
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<String>>()];
        for dir in paths.plan_dirs() {
            parts.push(
                dir.split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        }
        parts.push(
            paths
                .drafts
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        );
        parts
    }

    /// `--learn-from` 缺省时的语料发现（对齐 `corpus_paths_for_targets`）。
    pub fn corpus_paths_for_targets(&self, targets: &[PathBuf]) -> Vec<PathBuf> {
        let mut roots: Vec<(String, PathBuf)> = Vec::new();
        for path in targets {
            let parts: Vec<String> = path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            for marker in self.paths().corpus_markers() {
                let Some(idx) = parts.iter().position(|p| p == marker) else {
                    continue;
                };
                if idx > 0 {
                    let root = path.components().take(idx).collect::<PathBuf>();
                    let key = root.to_string_lossy().into_owned();
                    if !roots.iter().any(|(k, _)| k == &key) {
                        roots.push((key, root));
                    }
                }
                break;
            }
        }
        let mut corpus_paths = Vec::new();
        for (_, root) in &roots {
            for relative in self.default_corpus_parts() {
                let mut candidate = root.clone();
                for part in relative {
                    candidate.push(part);
                }
                if candidate.exists() {
                    corpus_paths.push(candidate);
                }
            }
        }
        corpus_paths
    }
}

mod analyze;
mod assemble;
mod collect;
mod dialogue;
mod fatigue;
mod output;
mod reminders;
mod report_markdown;
mod report_text;
mod scene;
mod sentence;

pub use analyze::{analyze_path, analyze_text, ReportFormat};
pub use collect::iter_target_files;
pub use dialogue::{build_character_voice_profile, build_dialogue_emotion_profile};
pub use fatigue::build_style_fatigue;
pub use output::{
    AaBbPattern, AbTurn, Analysis, BaContext, BaSample, BaselineJson, BattleProfile,
    BattleSequence, CharacterVoice, CorpusProfile, CorpusProfileJson, DialogueAxisGap,
    DialogueEmotions, DialogueReport, DominantPunctuation, EmotionSample, Ending, FatigueRow,
    FatigueWindow, HardFlag, JudgementContext, JudgementSample, LearnedAaBbShape,
    LearnedFilterMetric, LearnedPattern, LearnedSentenceLead, LearnedTermJson, ModifierPressure,
    OverlapEntry, PhraseCount, QuoteRun, ReviewReminder, SceneBlock, SceneMap,
    SentenceLengthBaseline, SentenceLengths, ShortQuoteRun, ShortRole, ShortRun, ShortSentence,
    SpeakerProfile, SpeakerSample, Summary, TemplateCandidate, TermCount, ToneProfile, ToneSample,
    TrackedTermWindow, TrackedTermWindowTerm, ViewpointProfile,
};
pub use reminders::build_review_reminders;
pub use report_markdown::{format_markdown_report, run, RunOptions};
pub use report_text::{format_text_report, render_report};
pub use scene::{
    build_battle_profile, build_scene_map, build_tone_profile, build_viewpoint_profile,
};
pub use sentence::{build_corpus_profile, build_learned_filter_metrics, collect_ngram_terms};

pub(crate) use analyze::{corpus_profile_json, first_snippet, AnalysisSections};
pub(crate) use assemble::assemble_analysis;
pub(crate) use collect::{
    build_ba_operation_contexts, build_tracked_term_windows, clean_corpus_text,
    format_tracked_term_counts, is_generated_or_template, is_whole_match,
};
pub(crate) use dialogue::{
    build_dialogue_axis_gaps, collect_judgement_contexts, detect_a_b_turns, detect_dialogue_runs,
    detect_question_ping_pong, detect_quote_ping_pong, detect_short_dialogue_runs,
    format_short_roles, suggest_short_run_action, summarize_short_roles,
};
pub(crate) use fatigue::{
    build_fatigue_windows, metric_evidence, regex_metrics_named, unique_evidence,
};
pub(crate) use report_text::path_stem;
pub(crate) use sentence::{
    build_sentence_length_profile, collect_aa_bb_patterns, collect_clause_prefixes,
    collect_connective_sentence_patterns, collect_judgement_endings, collect_modifier_pressure,
    collect_paragraph_leads, collect_parallel_clauses, collect_sentence_starts,
    collect_subject_leads,
};
