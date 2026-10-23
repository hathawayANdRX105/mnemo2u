//! Configuration for the novel craft.
//!
//! These settings describe the narrative memory itself — which directories hold
//! the corpus, and how a card declares its relations. They live with the craft
//! that consumes them rather than in whatever tool happens to drive it, so the
//! craft can be used from any binary.

use serde::{Deserialize, Serialize};

/// Where the corpus directories live inside one novel directory.
///
/// The craft never assumes the directory names: the caller says what this
/// novel calls them. The defaults match the layout the review rules ship with,
/// so a caller that does not care gets a working setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    /// Concept-card directory (may be nested).
    #[serde(default = "default_cards_dir")]
    pub cards: String,
    /// Draft directory.
    #[serde(default = "default_drafts_dir")]
    pub drafts: String,
    /// Arc-level outline directory.
    #[serde(default = "default_arc_plan_dir")]
    pub arc_plan: String,
    /// Story-outline directory (one file per story unit).
    #[serde(default = "default_story_plan_dir")]
    pub story_plan: String,
    /// Chapter-outline directory.
    #[serde(default = "default_chapter_plan_dir")]
    pub chapter_plan: String,
    /// Tool-output directory, relative to the novel directory: the consistency
    /// index and the three memory stores both land here.
    #[serde(default = "default_research_dir")]
    pub research: String,
}

impl Default for Paths {
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

impl Paths {
    /// Where the three memory stores go for one novel.
    #[must_use]
    pub fn memo_root(&self, novel_dir: &std::path::Path) -> std::path::PathBuf {
        novel_dir.join(&self.research).join("memo")
    }

    /// The glob pattern for concept cards.
    #[must_use]
    pub fn cards_glob(&self) -> String {
        format!("{}/**/*.md", self.cards)
    }
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

/// How the corpus is read and what counts as a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Local embedding model (a `fastembed` model name). Chinese fiction uses
    /// `BAAI/bge-m3` (1024 dimensions).
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Baseline for a fact's `created_at` (a millisecond timestamp).
    ///
    /// A fixed baseline rather than the current time: indexing is idempotent, so
    /// rescanning the same content must not look like a correction just because
    /// the clock moved. Facts after the baseline use `created_at_base` plus a
    /// revision counter.
    #[serde(default = "default_created_at_base")]
    pub created_at_base: i64,
    /// Section names in a story outline that count as facts.
    #[serde(default = "default_story_fact_sections")]
    pub story_fact_sections: Vec<String>,
    /// The card field that names related cards.
    #[serde(default = "default_card_related_field")]
    pub card_related_field: String,
    /// The card section that declares relations.
    #[serde(default = "default_card_relation_section")]
    pub card_relation_section: String,
}

impl Default for Config {
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
