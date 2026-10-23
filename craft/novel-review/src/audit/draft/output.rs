//! JSON 输出结构（字段名遵循 JSON 契约；`Analysis` 顶层键序固定）。

use super::*;

// ---------------------------------------------------------------------------
// JSON 输出结构（字段名遵循 JSON 契约）

/// 短句角色统计行。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShortRole {
    pub role: String,
    pub count: usize,
}

/// 短句连发块。
#[derive(Debug, Clone, Serialize)]
pub struct ShortRun {
    pub start_index: usize,
    pub end_index: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub avg_chars: f64,
    pub roles: Vec<ShortRole>,
    pub suggestion: String,
    pub sample: Vec<String>,
}

/// 单句长度信息。
#[derive(Debug, Clone, Serialize)]
pub struct ShortSentence {
    pub index: usize,
    pub line_no: usize,
    pub chars: usize,
    pub text: String,
}

/// `sentence_lengths` 节。
#[derive(Debug, Clone, Serialize)]
pub struct SentenceLengths {
    pub count: usize,
    pub min_chars: usize,
    pub p10_chars: usize,
    pub p25_chars: usize,
    pub median_chars: usize,
    pub avg_chars: f64,
    pub max_chars: usize,
    pub short_count: usize,
    pub very_short_count: usize,
    pub short_ratio: f64,
    pub warn: bool,
    pub short_sentences: Vec<ShortSentence>,
    pub very_short_sentences: Vec<ShortSentence>,
    pub short_runs: Vec<ShortRun>,
    pub sentences: Vec<ShortSentence>,
}

/// 局部疲劳窗口。
#[derive(Debug, Clone, Serialize)]
pub struct FatigueWindow {
    pub start_index: usize,
    pub end_index: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub score: usize,
    pub reasons: Vec<String>,
    pub roles: Vec<ShortRole>,
    pub suggestion: String,
    pub sample: Vec<String>,
    pub total_candidates: usize,
}

/// 对白转轴缺口。
#[derive(Debug, Clone, Serialize)]
pub struct DialogueAxisGap {
    pub start_index: usize,
    pub end_index: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub score: usize,
    pub reasons: Vec<String>,
    pub axes: Vec<String>,
    pub suggestion: String,
    pub sample: Vec<String>,
    pub total_candidates: usize,
}

/// 词计数行（judgement_contexts.top_terms）。
#[derive(Debug, Clone, Serialize)]
pub struct TermCount {
    pub term: String,
    pub count: usize,
}

/// 判断词样本。
#[derive(Debug, Clone, Serialize)]
pub struct JudgementSample {
    pub index: usize,
    pub line_no: usize,
    pub terms: Vec<String>,
    pub text: String,
}

/// `judgement_contexts` 行。
#[derive(Debug, Clone, Serialize)]
pub struct JudgementContext {
    pub context: String,
    pub label: String,
    pub count: usize,
    pub warn: bool,
    pub watch: bool,
    pub top_terms: Vec<TermCount>,
    pub samples: Vec<JudgementSample>,
}

/// 把字操作样本。
#[derive(Debug, Clone, Serialize)]
pub struct BaSample {
    pub index: usize,
    pub line_no: usize,
    pub snippet: String,
    pub sentence: String,
}

/// `ba_operation_contexts` 行。
#[derive(Debug, Clone, Serialize)]
pub struct BaContext {
    pub role: String,
    pub count: usize,
    pub samples: Vec<BaSample>,
    pub suggestion: String,
    pub warn: bool,
    pub total: usize,
}

/// 短语计数行（sentence_starts / clause_prefixes / parallel_clauses 等）。
#[derive(Debug, Clone, Serialize)]
pub struct PhraseCount {
    pub phrase: String,
    pub count: usize,
}

/// AA/BB 模式行。
#[derive(Debug, Clone, Serialize)]
pub struct AaBbPattern {
    #[serde(rename = "type")]
    pub pattern_type: String,
    pub name: String,
    pub count: usize,
    pub note: String,
    pub warn: bool,
    pub samples: Vec<String>,
}

/// 修饰/动作压力行。
#[derive(Debug, Clone, Serialize)]
pub struct ModifierPressure {
    pub label: String,
    pub total: usize,
    pub dense_sentences: usize,
    pub warn: bool,
}

/// 对白段连续引用块。
#[derive(Debug, Clone, Serialize)]
pub struct QuoteRun {
    pub start_paragraph: usize,
    pub end_paragraph: usize,
    pub sample: Vec<String>,
}

/// 短对白/引号乒乓块。
#[derive(Debug, Clone, Serialize)]
pub struct ShortQuoteRun {
    pub start_paragraph: usize,
    pub end_paragraph: usize,
    pub avg_len: f64,
    pub sample: Vec<String>,
}

/// A/B 交替说话人。
#[derive(Debug, Clone, Serialize)]
pub struct AbTurn {
    pub paragraph: usize,
    pub pattern: String,
}

/// `dialogue` 节。
#[derive(Debug, Clone, Serialize)]
pub struct DialogueReport {
    pub consecutive_quote_paragraph_runs: Vec<QuoteRun>,
    pub short_quote_runs: Vec<ShortQuoteRun>,
    pub question_ping_pong: Vec<QuoteRun>,
    pub quote_ping_pong: Vec<ShortQuoteRun>,
    pub dialogue_axis_gaps: Vec<DialogueAxisGap>,
    pub alternating_speaker_runs: Vec<AbTurn>,
    pub quote_paragraph_ratio: f64,
    pub dense_quote_run_max: usize,
    pub dense_quote_run_count: usize,
}

/// 场面功能块。
#[derive(Debug, Clone, Serialize)]
pub struct SceneBlock {
    pub role: String,
    pub start_paragraph: usize,
    pub end_paragraph: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub paragraphs: usize,
    pub chars: usize,
    pub sample: Vec<String>,
}

/// `scene_map` 节。
#[derive(Debug, Clone, Serialize)]
pub struct SceneMap {
    pub blocks: Vec<SceneBlock>,
    pub role_counts: CountMap,
    pub dominant_role: String,
    pub dominance_ratio: f64,
    pub warn: bool,
    pub block_count: usize,
    pub switch_count: usize,
}

/// 对白情绪样本。
#[derive(Debug, Clone, Serialize)]
pub struct EmotionSample {
    pub line_no: usize,
    pub label: String,
    pub labels: Vec<String>,
    pub text: String,
}

/// `dialogue_emotions` 节。
#[derive(Debug, Clone, Serialize)]
pub struct DialogueEmotions {
    pub dialogue_sentences: usize,
    pub emotion_counts: CountMap,
    pub dominant_emotion: String,
    pub dominant_ratio: f64,
    pub shift_count: usize,
    pub flatness_warn: bool,
    pub volatility_warn: bool,
    pub samples: Vec<EmotionSample>,
}

/// 角色对白画像样本。
#[derive(Debug, Clone, Serialize)]
pub struct SpeakerSample {
    pub line_no: usize,
    pub text: String,
    pub emotion: String,
}

/// 单个角色的对白画像。
#[derive(Debug, Clone, Serialize)]
pub struct SpeakerProfile {
    pub speaker: String,
    pub lines: usize,
    pub avg_chars: f64,
    pub question_ratio: f64,
    pub exclaim_ratio: f64,
    pub judgement_ratio: f64,
    pub short_ratio: f64,
    pub dominant_emotion: String,
    pub dominant_ratio: f64,
    pub samples: Vec<SpeakerSample>,
}

/// `character_voice` 节。
#[derive(Debug, Clone, Serialize)]
pub struct CharacterVoice {
    pub speaker_count: usize,
    pub identified_lines: usize,
    pub unknown_lines: usize,
    pub coverage_ratio: f64,
    pub dominant_speaker: String,
    pub dominant_ratio: f64,
    pub warn: bool,
    pub homogenized_pairs: Vec<String>,
    pub speakers: Vec<SpeakerProfile>,
}

/// 语气样本。
#[derive(Debug, Clone, Serialize)]
pub struct ToneSample {
    pub paragraph: usize,
    pub line_no: usize,
    pub tone: String,
    pub text: String,
}

/// `tone_profile` 节。
#[derive(Debug, Clone, Serialize)]
pub struct ToneProfile {
    pub tone_counts: CountMap,
    pub dominant_tone: String,
    pub stable_ratio: f64,
    pub switch_count: usize,
    pub samples: Vec<ToneSample>,
    pub warn: bool,
}

/// 战斗段序列。
#[derive(Debug, Clone, Serialize)]
pub struct BattleSequence {
    pub start_index: usize,
    pub end_index: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub sentences: usize,
    pub action_hits: usize,
    pub result_hits: usize,
    pub damage_hits: usize,
    pub movement_hits: usize,
    pub sample: Vec<String>,
    pub warn: bool,
}

/// `battle_profile` 节。
#[derive(Debug, Clone, Serialize)]
pub struct BattleProfile {
    pub sequence_count: usize,
    pub max_sequence_sentences: usize,
    pub action_hits: usize,
    pub result_hits: usize,
    pub damage_hits: usize,
    pub movement_hits: usize,
    pub result_ratio: f64,
    pub warn_sequences: usize,
    pub warn: bool,
    pub samples: Vec<BattleSequence>,
}

/// 视角重叠行。
#[derive(Debug, Clone, Serialize)]
pub struct OverlapEntry {
    pub paragraph: usize,
    pub line_no: usize,
    pub anchors: Vec<String>,
    pub text: String,
}

/// `viewpoint_profile` 节。
#[derive(Debug, Clone, Serialize)]
pub struct ViewpointProfile {
    pub anchor_counts: CountMap,
    pub dominant_anchor: String,
    pub switch_count: usize,
    pub overlap_count: usize,
    pub overlaps: Vec<OverlapEntry>,
    pub warn: bool,
}

/// `ending` 节。
#[derive(Debug, Clone, Serialize)]
pub struct Ending {
    pub tail_excerpt: String,
    pub image_terms: Vec<TermCount>,
    pub flow_terms: Vec<TermCount>,
    pub warn: bool,
}

/// `template_candidates` 行。
#[derive(Debug, Clone, Serialize)]
pub struct TemplateCandidate {
    #[serde(rename = "type")]
    pub candidate_type: String,
    pub name: String,
    pub count: usize,
    pub note: String,
    pub sample: String,
}

/// `hard_flags` 行（`per_10k` 可为 null）。
#[derive(Debug, Clone, Serialize)]
pub struct HardFlag {
    pub section: String,
    pub name: String,
    pub count: usize,
    pub per_10k: Option<f64>,
    pub note: String,
    /// 规则语域（`colloquial`/`literary`/`neutral`）；非规则来源（如
    /// learned_filters/tracked_terms）为 neutral。
    pub register: String,
    pub sample: String,
}

/// `style_fatigue` 行。
#[derive(Debug, Clone, Serialize)]
pub struct FatigueRow {
    pub family: String,
    pub status: String,
    pub count: usize,
    pub risk: String,
    pub reduce: String,
    pub evidence: Vec<String>,
}

/// `review_reminders` 行。
#[derive(Debug, Clone, Serialize)]
pub struct ReviewReminder {
    pub priority: String,
    pub category: String,
    pub title: String,
    pub reason: String,
    pub check: String,
    pub action: String,
    pub evidence: Vec<String>,
}

/// 语料学习得到的模式。
#[derive(Debug, Clone, Serialize)]
pub struct LearnedPattern {
    pub category: String,
    pub name: String,
    pub count: usize,
    pub corpus_per_10k: f64,
    pub max_per_10k: f64,
    pub note: String,
}

/// 语料学习得到的句首。
#[derive(Debug, Clone, Serialize)]
pub struct LearnedSentenceLead {
    pub phrase: String,
    pub count: usize,
    pub corpus_per_10k: f64,
}

/// 语料学习得到的 AA/BB 形状。
#[derive(Debug, Clone, Serialize)]
pub struct LearnedAaBbShape {
    pub name: String,
    pub count: usize,
    pub note: String,
}

/// 句长基线（`corpus_profile.sentence_length_baseline`）。
#[derive(Debug, Clone, Serialize)]
pub struct SentenceLengthBaseline {
    pub sentence_count: usize,
    pub p10_chars: usize,
    pub p25_chars: usize,
    pub median_chars: usize,
    pub avg_chars: f64,
    pub short_ratio: f64,
}

/// 语料画像（`corpus_profile` 节的数据来源；None 时输出禁用默认值）。
#[derive(Debug, Clone)]
pub struct CorpusProfile {
    pub source_count: usize,
    pub chars: usize,
    pub draft_chars: usize,
    pub learned_terms: Vec<LearnedPattern>,
    pub learned_style_phrases: Vec<LearnedPattern>,
    pub learned_sentence_leads: Vec<LearnedSentenceLead>,
    pub learned_aa_bb_shapes: Vec<LearnedAaBbShape>,
    pub sentence_length_baseline: Option<SentenceLengthBaseline>,
}

/// JSON 里 `sentence_length_baseline` 的两种形态：
/// 未启用语料学习 → `{}`（空对象）；语料存在 → 6 键基线对象。
#[derive(Debug, Clone)]
pub enum BaselineJson {
    Empty,
    Values(SentenceLengthBaseline),
}

impl Serialize for BaselineJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Empty => {
                let map = serializer.serialize_map(Some(0))?;
                serde::ser::SerializeMap::end(map)
            }
            Self::Values(values) => values.serialize(serializer),
        }
    }
}

/// `corpus_profile` 节。
#[derive(Debug, Clone, Serialize)]
pub struct CorpusProfileJson {
    pub enabled: bool,
    pub source_count: usize,
    pub chars: usize,
    pub draft_chars: usize,
    pub learned_terms: Vec<LearnedTermJson>,
    pub learned_style_phrases: Vec<LearnedTermJson>,
    pub learned_sentence_leads: Vec<LearnedSentenceLead>,
    pub learned_aa_bb_shapes: Vec<LearnedAaBbShape>,
    pub sentence_length_baseline: BaselineJson,
}

/// 语料高频词 JSON 行。
#[derive(Debug, Clone, Serialize)]
pub struct LearnedTermJson {
    pub name: String,
    pub category: String,
    pub count: usize,
    pub corpus_per_10k: f64,
    pub max_per_10k: f64,
}

/// 语料高频词/句法手势指标行（`learned_filters` 节）。
#[derive(Debug, Clone, Serialize)]
pub struct LearnedFilterMetric {
    pub category: String,
    pub name: String,
    pub count: usize,
    pub per_10k: f64,
    pub corpus_per_10k: f64,
    pub max_per_10k: f64,
    pub note: String,
    pub warn: bool,
    pub samples: Vec<Hit>,
}

/// 跟踪词窗口局部行。
#[derive(Debug, Clone, Serialize)]
pub struct TrackedTermWindowTerm {
    pub term: String,
    pub count: usize,
    pub category: String,
    pub note: String,
}

/// `tracked_term_windows` 行。
#[derive(Debug, Clone, Serialize)]
pub struct TrackedTermWindow {
    pub start_index: usize,
    pub end_index: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub window_size: usize,
    pub score: usize,
    pub total_hits: usize,
    pub top_term: String,
    pub top_count: usize,
    pub top_category: String,
    pub reasons: Vec<String>,
    pub terms: Vec<TrackedTermWindowTerm>,
    pub suggestion: String,
    pub sample: Vec<String>,
    pub total_candidates: usize,
}

/// 主要标点（`dominant_punctuation` 行）。
#[derive(Debug, Clone, Serialize)]
pub struct DominantPunctuation {
    pub mark: String,
    pub count: usize,
    pub per_10k: f64,
}

/// `summary` 节。
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub chars: usize,
    pub sentences: usize,
    pub paragraphs: usize,
    pub avg_sentence_chars: f64,
    pub short_sentences: usize,
    pub very_short_sentences: usize,
    pub short_sentence_ratio: f64,
    pub quote_ratio: f64,
    pub warn_sections: usize,
}

/// 完整 analysis 结构（顶层键序固定）。
#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub source: String,
    pub summary: Summary,
    pub warned: bool,
    pub tokens: Vec<RegexMetric>,
    pub tracked_terms: Vec<crate::rules::TrackedMetric>,
    pub tracked_term_categories: Vec<crate::rules::CategoryRow>,
    pub tracked_term_windows: Vec<TrackedTermWindow>,
    pub tracked_term_window_count: usize,
    pub ba_operation_contexts: Vec<BaContext>,
    pub patterns: Vec<RegexMetric>,
    pub phrases: Vec<RegexMetric>,
    pub modifiers: Vec<RegexMetric>,
    pub punctuation: Vec<RegexMetric>,
    pub punctuation_combos: Vec<RegexMetric>,
    pub custom_templates: Vec<CustomTemplateMetric>,
    pub learned_filters: Vec<LearnedFilterMetric>,
    pub corpus_profile: CorpusProfileJson,
    pub dominant_punctuation: Vec<DominantPunctuation>,
    pub sentence_starts: Vec<PhraseCount>,
    pub subject_leads: Vec<PhraseCount>,
    pub paragraph_leads: Vec<PhraseCount>,
    pub sentence_patterns: Vec<PhraseCount>,
    pub judgement_endings: Vec<PhraseCount>,
    pub clause_prefixes: Vec<PhraseCount>,
    pub parallel_clauses: Vec<PhraseCount>,
    pub aa_bb_patterns: Vec<AaBbPattern>,
    pub sentence_lengths: SentenceLengths,
    pub fatigue_windows: Vec<FatigueWindow>,
    pub fatigue_window_count: usize,
    pub judgement_contexts: Vec<JudgementContext>,
    pub modifier_pressure: Vec<ModifierPressure>,
    pub terms: Vec<TermCount>,
    pub short_phrases: Vec<TermCount>,
    pub dialogue: DialogueReport,
    pub scene_map: SceneMap,
    pub dialogue_emotions: DialogueEmotions,
    pub character_voice: CharacterVoice,
    pub tone_profile: ToneProfile,
    pub battle_profile: BattleProfile,
    pub viewpoint_profile: ViewpointProfile,
    pub ending: Ending,
    pub template_candidates: Vec<TemplateCandidate>,
    pub hard_flags: Vec<HardFlag>,
    pub style_fatigue: Vec<FatigueRow>,
    pub review_reminders: Vec<ReviewReminder>,
}
