//! 规则配置加载。
//!
//! 把 `configs/rules/review.yaml` 解析为强类型结构（`load` / `mapping_at` /
//! `list_at`）：缺失关键节或 YAML 非法时返回错误。
//! 未知字段被 serde 忽略（宽容解析）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// 默认规则文件路径：`<repo root>/configs/rules/review.yaml`。
pub fn default_rules_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/rules/review.yaml")
}

/// 规则文件路径：给了就用给的，没给就用默认。
///
/// Every command that reads rules goes through this, so `--rules` on the
/// command line is the only thing that decides which file is loaded.
#[must_use]
pub fn resolve_rules_path(rules: Option<&Path>) -> PathBuf {
    match rules {
        Some(path) => path.to_path_buf(),
        None => default_rules_path(),
    }
}

/// 读取并解析规则文件。
pub fn load_rules(path: &Path) -> Result<ReviewRules> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取规则文件失败: {}", path.display()))?;
    let rules: ReviewRules = serde_yaml::from_str(&content)
        .with_context(|| format!("解析规则 YAML 失败: {}", path.display()))?;
    Ok(rules)
}

/// 读取规则文件；`rules` 给了就用它，没给就用默认路径。
pub fn load_rules_from(rules: Option<&Path>) -> Result<ReviewRules> {
    load_rules(&resolve_rules_path(rules))
}

/// 读取规则文件里的 `paths` 节；读不到时用内置默认值。
///
/// 只有目录布局的调用方需要它（一致性索引的 glob），它们不必加载整份规则。
pub fn load_paths(path: &Path) -> PathsConfig {
    load_rules(path)
        .map(|rules| rules.paths)
        .unwrap_or_default()
}

/// 内置的默认目录布局。
#[must_use]
pub fn default_paths() -> PathsConfig {
    PathsConfig::default()
}

/// 规则根结构，对应 review.yaml 顶层 `draft` / `plan`。
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewRules {
    pub draft: DraftConfig,
    pub plan: PlanConfig,
    #[serde(default)]
    pub study: Option<StudyConfig>,
    /// 小说目录布局，对应顶层 `paths` 节。
    #[serde(default)]
    pub paths: PathsConfig,
    /// 记忆层配置，对应顶层 `memory` 节。
    #[serde(default)]
    pub memory: MemoryConfig,
}

/// 记忆层配置，对应 `memory` 节。
///
/// 记忆层把设定 / 事实 / 关系三层写进 mnemo2u（三库），时间轴是 story 时间
/// 而不是写作顺序——插叙、倒序的章节号不能当时间轴用。
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfig {
    /// 本地嵌入模型（fastembed 的模型名）。中文小说用 bge-m3（1024 维）。
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// 事实的 `created_at` 基线（毫秒时间戳）。
    ///
    /// 用固定基线而不是当前时间：索引是幂等的，重扫同一份内容不应该因为
    /// 时间戳变了就被判成"修正"。基线之后的事实用 `created_at_base` 加上
    /// 修订序号。
    #[serde(default = "default_created_at_base")]
    pub created_at_base: i64,
    /// story 大纲里算作事实的小节名。
    #[serde(default = "default_story_fact_sections")]
    pub story_fact_sections: Vec<String>,
    /// 概念卡里写关联卡片的字段名。
    #[serde(default = "default_card_related_field")]
    pub card_related_field: String,
    /// 概念卡里声明关系的小节名。
    #[serde(default = "default_card_relation_section")]
    pub card_relation_section: String,
}

fn default_embedding_model() -> String {
    "BAAI/bge-m3".to_string()
}

fn default_created_at_base() -> i64 {
    0
}

fn default_story_fact_sections() -> Vec<String> {
    vec![
        "核心事件".to_string(),
        "粗章节负载".to_string(),
        "主要角色与功能".to_string(),
    ]
}

fn default_card_related_field() -> String {
    "关联卡片".to_string()
}

fn default_card_relation_section() -> String {
    "关系".to_string()
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            embedding_model: default_embedding_model(),
            created_at_base: default_created_at_base(),
            story_fact_sections: default_story_fact_sections(),
            card_related_field: default_card_related_field(),
            card_relation_section: default_card_relation_section(),
        }
    }
}

/// 小说目录下的子目录名。
///
/// 每项独立可配，缺省时与内置默认值相同——不写 `paths` 节的行为与
/// 写全默认值的行为一致。目录名可以是多级（如 `concept/cards`），匹配
/// 时按路径分量逐段比。
#[derive(Debug, Clone, Deserialize)]
pub struct PathsConfig {
    /// 概念卡目录。
    #[serde(default = "default_cards_dir")]
    pub cards: String,
    /// 正文目录。
    #[serde(default = "default_drafts_dir")]
    pub drafts: String,
    /// Arc 级大纲目录。
    #[serde(default = "default_arc_plan_dir")]
    pub arc_plan: String,
    /// Story 级大纲目录。
    #[serde(default = "default_story_plan_dir")]
    pub story_plan: String,
    /// 章级大纲目录。
    #[serde(default = "default_chapter_plan_dir")]
    pub chapter_plan: String,
    /// 工具产物目录（相对小说目录）：一致性索引与记忆层三库都落在这里。
    #[serde(default = "default_research_dir")]
    pub research: String,
}

fn default_cards_dir() -> String {
    "concept/cards".to_string()
}

fn default_drafts_dir() -> String {
    "drafts".to_string()
}

fn default_arc_plan_dir() -> String {
    "arc-plan".to_string()
}

fn default_story_plan_dir() -> String {
    "story-plan".to_string()
}

fn default_chapter_plan_dir() -> String {
    "chapter-plan".to_string()
}

fn default_research_dir() -> String {
    "research".to_string()
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            cards: default_cards_dir(),
            drafts: default_drafts_dir(),
            arc_plan: default_arc_plan_dir(),
            story_plan: default_story_plan_dir(),
            chapter_plan: default_chapter_plan_dir(),
            research: default_research_dir(),
        }
    }
}

impl PathsConfig {
    /// 三类大纲目录名，按 arc/story/chapter 顺序。
    #[must_use]
    pub fn plan_dirs(&self) -> [&str; 3] {
        [&self.arc_plan, &self.story_plan, &self.chapter_plan]
    }

    /// 某份 plan 文件属于哪一层：按配置的目录名判定，路径里含哪个
    /// 目录名就是哪一层。
    #[must_use]
    pub fn plan_type_of(&self, path: &Path) -> Option<&'static str> {
        let joined = path.to_string_lossy();
        let candidates = [
            ("arc-plan", &self.arc_plan),
            ("story-plan", &self.story_plan),
            ("chapter-plan", &self.chapter_plan),
        ];
        candidates
            .iter()
            .find(|(_, dir)| !dir.is_empty() && joined.contains(dir.as_str()))
            .map(|(kind, _)| *kind)
    }

    /// 路径的哪一段是配置里的某个计划目录名（用于镜像路径计算）。
    ///
    /// 目录名可以是多级（`plans/chapters`），按连续分量序列匹配，命中后
    /// 取最后一段的位置。
    #[must_use]
    pub fn plan_dir_component(&self, path: &Path) -> Option<usize> {
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        self.plan_dirs()
            .iter()
            .find_map(|dir| match_dir_components(&parts, dir))
    }

    /// 正文目录在路径中的最后一段位置（用于镜像路径计算）。
    ///
    /// 命中的是「这个目录就是正文目录」——不是「路径里含正文目录」。两者
    /// 在嵌套布局下不同：`drafts/arc1/story1` 的 story 目录向上找到
    /// `drafts/arc1` 时，若按「含」判定就会把 arc 层误当正文目录。
    #[must_use]
    pub fn drafts_dir_component(&self, path: &Path) -> Option<usize> {
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        match_dir_components(&parts, &self.drafts)
    }

    /// 这个目录**本身就是**正文目录时，返回它在路径中的位置。
    ///
    /// 与 [`Self::drafts_dir_component`] 的区别：那个回答「路径落在正文
    /// 目录下吗」（文件路径也算），这个回答「这个目录是正文目录吗」。嵌套
    /// 布局下两者不同——`drafts/arc1/story1` 的祖先里，`drafts` 才是正文
    /// 目录，`drafts/arc1` 只是它下面的 arc 层。
    #[must_use]
    pub fn drafts_root_component(&self, dir: &Path) -> Option<usize> {
        let parts: Vec<String> = dir
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let segments = self.drafts.split('/').filter(|s| !s.is_empty()).count();
        match_dir_components(&parts, &self.drafts).filter(|idx| idx + segments == parts.len())
    }

    /// 概念卡目录在路径中的最后一段位置。
    #[must_use]
    pub fn cards_dir_component(&self, path: &Path) -> Option<usize> {
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        match_dir_components(&parts, &self.cards)
    }

    /// 一致性索引的数据库路径（相对小说目录）。
    #[must_use]
    pub fn consistency_db_path(&self, novel_dir: &Path) -> PathBuf {
        novel_dir
            .join(&self.research)
            .join("consistency")
            .join("consistency.sqlite3")
    }

    /// 记忆层三库的根目录（相对小说目录）。
    #[must_use]
    pub fn memo_root(&self, novel_dir: &Path) -> PathBuf {
        novel_dir.join(&self.research).join("memo")
    }

    /// 概念卡目录的 glob 模式。
    #[must_use]
    pub fn cards_glob(&self) -> String {
        format!("{}/**/*.md", self.cards)
    }

    /// 索引文档的 glob 模式列表（概念卡 + 三类大纲 + 正文）。
    #[must_use]
    pub fn doc_globs(&self) -> Vec<String> {
        let mut globs = vec![self.cards_glob()];
        for dir in self.plan_dirs() {
            globs.push(format!("{dir}/**/*.md"));
        }
        globs.push(format!("{}/**/*.md", self.drafts));
        globs
    }

    /// 语料自动发现用的目录名标记。
    ///
    /// 每个标记是目录名的最后一段（`concept/cards` → `cards`）——语料发现
    /// 只需要在路径里认出"这是哪个目录"，找到后取它之前的一段当小说根。
    #[must_use]
    pub fn corpus_markers(&self) -> Vec<&str> {
        let mut markers = vec![last_segment(&self.drafts)];
        markers.push(last_segment(&self.cards));
        for dir in self.plan_dirs() {
            markers.push(last_segment(dir));
        }
        markers
    }
}

/// 目录名的最后一段（`concept/cards` → `cards`）。
fn last_segment(dir: &str) -> &str {
    dir.rsplit('/').find(|s| !s.is_empty()).unwrap_or(dir)
}

/// 在 `parts` 里找 `dir` 的连续分量序列，返回最后一段的下标。
///
/// `dir` 可以是单级（`drafts`）或多级（`concept/cards`）。多级时必须
/// 整段连续命中，否则 `cards` 会误配到 `concept/cards` 之外的名字上。
fn match_dir_components(parts: &[String], dir: &str) -> Option<usize> {
    let wanted: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    if wanted.is_empty() {
        return None;
    }
    if wanted.len() > parts.len() {
        return None;
    }
    (0..=parts.len() - wanted.len()).find(|&start| {
        parts[start..start + wanted.len()]
            .iter()
            .zip(&wanted)
            .all(|(part, want)| part == want)
    })
}

/// 草稿规则，对应 `draft` 节。
#[derive(Debug, Clone, Deserialize)]
pub struct DraftConfig {
    pub regex_rules: RegexRules,
    pub template_rules: Vec<TemplateRule>,
    pub inactive_template_candidates: Vec<TemplateRule>,
    pub tracked_terms: Vec<TrackedTerm>,
    pub thresholds: DraftThresholds,
    pub learned_term_window: LearnedTermWindow,
    pub markdown_noise_line: MarkdownNoiseLine,
    pub connective_sentence_patterns: Vec<ConnectivePattern>,
    pub speaker: SpeakerConfig,
    pub lexicon: Lexicon,
    pub ending_labels: EndingLabels,
}

/// 正则规则分组，对应 `draft.regex_rules`。
#[derive(Debug, Clone, Deserialize)]
pub struct RegexRules {
    pub tokens: Vec<RegexRule>,
    pub patterns: Vec<RegexRule>,
    pub phrases: Vec<RegexRule>,
    pub modifiers: Vec<RegexRule>,
    pub punctuation: Vec<RegexRule>,
    pub punctuation_combos: Vec<RegexRule>,
}

/// 单条正则规则（高频词/句式/标点等）。
#[derive(Debug, Clone, Deserialize)]
pub struct RegexRule {
    pub name: String,
    pub pattern: String,
    /// 每万字上限。
    pub max_per_10k: f64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// 语域标签（可逗号组合多个）：`colloquial`（口语）/ `literary`（文学、
    /// 含散文）/ `classical`（文白古典）/ `lightnovel`（日轻翻译腔）/
    /// `webnovel`（网文）/ `common`（各人类语域通用）/ `neutral`（无语域倾向）。
    /// 来自真语料矩阵校验：这些规则的命中在对应语域是人类正常用法，
    /// 而不是 AI 腔证据（如标点「：」「！」在网文/古典/文学里都高频，
    /// 而 42 章 AI 草稿零命中）。报告携带此标签，让评审按作品实际语域
    /// 判读，而不是一律当 AI 味。
    #[serde(default)]
    pub register: Option<String>,
}

/// 模板规则，同时用于 `template_rules` 与 `inactive_template_candidates`。
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateRule {
    pub name: String,
    pub pattern: String,
    pub note: String,
    pub max_per_10k: f64,
    pub category: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// 跟踪词（人名/地名/动作等）。
#[derive(Debug, Clone, Deserialize)]
pub struct TrackedTerm {
    pub category: String,
    pub term: String,
    pub max_per_10k: f64,
    #[serde(default)]
    pub note: Option<String>,
}

/// 草稿阈值，对应 `draft.thresholds`。
#[derive(Debug, Clone, Deserialize)]
pub struct DraftThresholds {
    pub short_sentence_max_chars: u32,
    pub very_short_sentence_max_chars: u32,
    pub short_sentence_run_max_chars: u32,
    pub short_sentence_run_min: u32,
    pub tracked_term_window_size: u32,
    pub tracked_term_window_min_top: u32,
    pub tracked_term_window_min_total: u32,
    pub tracked_term_window_min_category: u32,
    pub learned_filter_limit: u32,
}

/// 语料学习窗口配置，对应 `draft.learned_term_window`。
#[derive(Debug, Clone, Deserialize)]
pub struct LearnedTermWindow {
    pub categories: Vec<String>,
    pub noise_suffixes: Vec<String>,
    pub noise_prefixes: Vec<String>,
    pub noise_chars: Vec<String>,
    /// 归因句尾缀：分句以此为结尾时视为话语归因（「段誉道」「他说」），
    /// 不参与 `clause_prefixes` 分句骨架重复统计。来自扩展真语料矩阵：
    /// 归因形态在人写经典（天龙 55%/围城 50%）与 AI 章（约 40%）占比相近，
    /// 无判别力，却是人写长篇 learned 标记的最大来源（天龙 22 条里 16 条）。
    #[serde(default)]
    pub attribution_suffixes: Vec<String>,
}

/// Markdown 噪音行正则，对应 `draft.markdown_noise_line`。
#[derive(Debug, Clone, Deserialize)]
pub struct MarkdownNoiseLine {
    pub pattern: String,
}

/// 连接词句首模式，对应 `draft.connective_sentence_patterns`。
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectivePattern {
    pub label: String,
    pub pattern: String,
}

/// 说话人识别配置，对应 `draft.speaker`。
#[derive(Debug, Clone, Deserialize)]
pub struct SpeakerConfig {
    pub patterns: Vec<String>,
    pub line_patterns: Vec<String>,
    pub suffixes: Vec<String>,
}

/// 词库，对应 `draft.lexicon`。绝大多数是词表，个别是字符串或标签→词表映射。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Lexicon {
    #[serde(default)]
    pub word_stoplist: Vec<String>,
    #[serde(default)]
    pub corpus_stop_terms: Vec<String>,
    #[serde(default)]
    pub structure_chars: String,
    #[serde(default)]
    pub subject_leads: Vec<String>,
    #[serde(default)]
    pub paragraph_leads: Vec<String>,
    #[serde(default)]
    pub allowed_short_corpus_terms: Vec<String>,
    #[serde(default)]
    pub ending_image_terms: Vec<String>,
    #[serde(default)]
    pub ending_flow_terms: Vec<String>,
    #[serde(default)]
    pub judgement_endings: Vec<String>,
    #[serde(default)]
    pub fatigue_window_judgement_terms: Vec<String>,
    #[serde(default)]
    pub fatigue_window_sticky_terms: Vec<String>,
    #[serde(default)]
    pub judgement_context_terms: Vec<String>,
    #[serde(default)]
    pub short_role_info_terms: Vec<String>,
    #[serde(default)]
    pub short_role_emotion_terms: Vec<String>,
    #[serde(default)]
    pub ba_clue_terms: Vec<String>,
    #[serde(default)]
    pub ba_emotion_terms: Vec<String>,
    #[serde(default)]
    pub ba_scene_terms: Vec<String>,
    #[serde(default)]
    pub ba_scene_verbs: Vec<String>,
    #[serde(default)]
    pub ba_tool_terms: Vec<String>,
    #[serde(default)]
    pub dialogue_axis_action_terms: Vec<String>,
    #[serde(default)]
    pub dialogue_axis_env_terms: Vec<String>,
    #[serde(default)]
    pub dialogue_axis_device_terms: Vec<String>,
    #[serde(default)]
    pub dialogue_axis_third_party_terms: Vec<String>,
    #[serde(default)]
    pub adjective_hints: Vec<String>,
    #[serde(default)]
    pub verb_hints: Vec<String>,
    #[serde(default)]
    pub scene_break_leads: Vec<String>,
    #[serde(default)]
    pub paragraph_info_terms: Vec<String>,
    #[serde(default)]
    pub mental_state_terms: Vec<String>,
    #[serde(default, deserialize_with = "ordered_term_rules_deserialize")]
    pub dialogue_emotion_rules: OrderedTermRules,
    #[serde(default)]
    pub character_name_stoplist: Vec<String>,
    #[serde(default, deserialize_with = "ordered_term_rules_deserialize")]
    pub tone_rules: OrderedTermRules,
    #[serde(default)]
    pub battle_action_terms: Vec<String>,
    #[serde(default)]
    pub battle_result_terms: Vec<String>,
    #[serde(default)]
    pub battle_damage_terms: Vec<String>,
    #[serde(default)]
    pub battle_movement_terms: Vec<String>,
}

/// 保留 YAML 插入序的「标签 → 词表」映射。
///
/// `dialogue_emotion_rules` / `tone_rules` 的首匹配语义依赖 YAML 里的书写顺序
/// （规则顺序 = YAML 插入序）。`serde_yaml::Mapping`
/// 保留插入序，据此直接转有序 `Vec` 对；不能用 `HashMap`（丢序），也不能
/// 直接声明 `Vec<(K, V)>`（YAML 值是 mapping，不是 sequence，反序列化失败）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrderedTermRules {
    pub rules: Vec<(String, Vec<String>)>,
}

impl<'de> serde::Deserialize<'de> for OrderedTermRules {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        match value {
            serde_yaml::Value::Mapping(map) => {
                let mut rules = Vec::with_capacity(map.len());
                for (key, val) in map.iter() {
                    let label = key
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| key.as_u64().map(|n| n.to_string()))
                        .or_else(|| key.as_bool().map(|b| b.to_string()))
                        .unwrap_or_default();
                    let terms = match val {
                        serde_yaml::Value::Sequence(items) => items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect(),
                        _ => Vec::new(),
                    };
                    rules.push((label, terms));
                }
                Ok(Self { rules })
            }
            serde_yaml::Value::Null => Ok(Self { rules: Vec::new() }),
            other => Err(serde::de::Error::custom(format!(
                "期望 mapping 或 null，实际为: {other:?}"
            ))),
        }
    }
}

/// `OrderedTermRules` 的 `serde(deserialize_with)` 入口。
fn ordered_term_rules_deserialize<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<OrderedTermRules, D::Error> {
    OrderedTermRules::deserialize(deserializer)
}

/// 章末收束标签，对应 `draft.ending_labels`。
#[derive(Debug, Clone, Deserialize)]
pub struct EndingLabels {
    pub rules: HashMap<String, Vec<String>>,
    pub display: HashMap<String, String>,
}

/// 大纲规则，对应 `plan` 节。
#[derive(Debug, Clone, Deserialize)]
pub struct PlanConfig {
    pub required_headings: HashMap<String, Vec<Vec<String>>>,
    pub scene_field_groups: Vec<Vec<String>>,
    pub chapter_ending_group: Vec<String>,
    pub sections: PlanSections,
    pub thresholds: PlanThresholds,
    pub regex: HashMap<String, PlanRegexRule>,
    pub lookpoint_weak_terms: Vec<String>,
    pub lookpoint_strong_terms: Vec<String>,
    pub ending_weak_terms: Vec<String>,
    pub environment_pressure_terms: Vec<String>,
    pub environment_witness_terms: Vec<String>,
    pub generic_progress_terms: Vec<String>,
    pub role_function_terms: Vec<String>,
    pub function_rules: FunctionRules,
}

/// 大纲节名配置，对应 `plan.sections`。
#[derive(Debug, Clone, Deserialize)]
pub struct PlanSections {
    pub story_layout: String,
    pub foreshadow_table: String,
    pub story_events: String,
    pub story_loads: String,
    pub story_roles: String,
    pub story_environment: Vec<String>,
    pub chapter_function: String,
    pub lookpoint: String,
    pub rhythm: String,
    pub state_change: String,
    pub scene_function_fields: Vec<String>,
}

/// 大纲阈值，对应 `plan.thresholds`。
#[derive(Debug, Clone, Deserialize)]
pub struct PlanThresholds {
    pub story_layout_min_items: u32,
    pub foreshadow_min_ids: u32,
    pub story_events_min_items: u32,
    pub lookpoint_short_max_chars: u32,
    pub scene_body_max_chars: u32,
    pub scene_prose_mark_min: u32,
    pub scene_dialogue_prose_mark_min: u32,
    pub thin_change_max_chars: u32,
    pub scene_monotony_min_scenes: u32,
    pub scene_monotony_max_missing: u32,
}

/// 单条大纲正则，对应 `plan.regex.<name>`。
#[derive(Debug, Clone, Deserialize)]
pub struct PlanRegexRule {
    pub pattern: String,
    #[serde(default)]
    pub ignore_case: bool,
}

/// 功能规则，对应 `plan.function_rules`。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FunctionRules {
    #[serde(default)]
    pub chapter: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub ending: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub scene: HashMap<String, Vec<String>>,
}

/// 研究工具词集配置，对应 `study` 节（可选；缺省时各词集为空）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct StudyConfig {
    /// 代名词表（pronouns）。
    pub pronouns: Vec<String>,
    /// 人名表（personal_names）。
    pub personal_names: Vec<String>,
    /// 对白归属词表（dialogue_attribution）。
    pub dialogue_attribution: Vec<String>,
}
