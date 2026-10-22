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
/// novel calls them, exactly as the review rules do for the corpus.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    /// Concept-card directory (may be nested).
    pub cards: String,
    /// Draft directory.
    pub drafts: String,
    /// Story-outline directory (one file per story unit).
    pub story_plan: String,
    /// Chapter-outline directory.
    pub chapter_plan: String,
    /// Where the three memory stores are written, relative to the novel directory.
    pub memo_root: String,
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
    1_700_000_000_000
}

fn default_story_fact_sections() -> Vec<String> {
    vec!["事实".to_string(), "Facts".to_string()]
}

fn default_card_related_field() -> String {
    "关联卡片".to_string()
}

fn default_card_relation_section() -> String {
    "关系".to_string()
}
