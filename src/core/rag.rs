//! RAG-domain types for the nano-graphrag port (R1).
//!
//! Field names and semantics mirror the reference implementation
//! (`refs/nano-graphrag/nano_graphrag/base.py`, `_op.py`) so golden fixtures
//! generated from Python can be compared field by field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `full_docs` KV value: the raw document text (`GraphRAG.ainsert`, graphrag.py:279).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocRecord {
    pub content: String,
}

/// `text_chunks` KV value (`base.py:32-35` TextChunkSchema).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub tokens: usize,
    pub content: String,
    pub chunk_order_index: usize,
    pub full_doc_id: String,
    /// Owning document path — LightRAG carries it on the chunk
    /// (`operate.py:4105`) and every extraction record inherits it.
    #[serde(default = "default_file_path")]
    pub file_path: String,
}

/// One extracted entity (`_op.py::_handle_single_entity_extraction`, :138-156).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityRecord {
    pub entity_name: String,
    pub entity_type: String,
    pub description: String,
    pub source_id: String,
    /// LightRAG carries the owning document path on every record
    /// (`operate.py::_handle_single_entity_extraction`, default
    /// `"unknown_source"`); the graph node upsert writes it verbatim.
    #[serde(default = "default_file_path")]
    pub file_path: String,
    /// Extraction wall-clock time (unix seconds); the graph node `created_at`
    /// field reads it back (`operate.py:1791`).
    #[serde(default)]
    pub timestamp: i64,
}

/// One extracted relation (`_op.py::_handle_single_relationship_extraction`, :159-179).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationRecord {
    pub src_id: String,
    pub tgt_id: String,
    pub weight: f64,
    pub description: String,
    pub source_id: String,
    /// Only produced by DSPy predictions; defaults to 1 (`_op.py:261`).
    pub order: i64,
    /// High-level keywords for this relation (LightRAG tuple field 3,
    /// `operate.py:814-816`); joined with commas and stored on the graph edge
    /// and in `relationships_vdb`.
    #[serde(default)]
    pub keywords: String,
    /// Owning document path (`operate.py::_handle_single_relationship_extraction`).
    #[serde(default = "default_file_path")]
    pub file_path: String,
    /// Extraction wall-clock time (unix seconds).
    #[serde(default)]
    pub timestamp: i64,
}

fn default_file_path() -> String {
    "unknown_source".to_string()
}

/// `DEFAULT_TOP_K` (`constants.py:57`); entity arm depth.
pub const DEFAULT_TOP_K: usize = 40;
/// `DEFAULT_CHUNK_TOP_K` (`constants.py:58`).
pub const DEFAULT_CHUNK_TOP_K: usize = 20;
/// `DEFAULT_MAX_ENTITY_TOKENS` (`constants.py:59`).
pub const DEFAULT_MAX_ENTITY_TOKENS: usize = 6_000;
/// `DEFAULT_MAX_RELATION_TOKENS` (`constants.py:60`).
pub const DEFAULT_MAX_RELATION_TOKENS: usize = 8_000;
/// `DEFAULT_MAX_TOTAL_TOKENS` (`constants.py:61`).
pub const DEFAULT_MAX_TOTAL_TOKENS: usize = 30_000;
/// `DEFAULT_COSINE_THRESHOLD` (`constants.py:62`).
pub const DEFAULT_COSINE_THRESHOLD: f32 = 0.2;
/// `DEFAULT_RELATED_CHUNK_NUMBER` (`constants.py:63`).
pub const DEFAULT_RELATED_CHUNK_NUMBER: usize = 5;

/// Community record (`base.py:38-56` SingleCommunitySchema/CommunitySchema).
/// `report_string`/`report_json` are `None` until the report pass runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommunitySchema {
    pub level: i64,
    pub title: String,
    pub edges: Vec<(String, String)>,
    pub nodes: Vec<String>,
    pub chunk_ids: Vec<String>,
    pub occurrence: f64,
    pub sub_communities: Vec<String>,
    #[serde(default)]
    pub report_string: Option<String>,
    #[serde(default)]
    pub report_json: Option<Value>,
}

/// Query mode (`base.py:11` Literal["local", "global", "naive"]; LightRAG adds
/// "hybrid"/"mix", `lightrag/base.py:93`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryMode {
    Local,
    Global,
    Naive,
    /// Both keyword arms run and their results interleave (`kg_query`,
    /// operate.py:5366-5461).
    Hybrid,
    /// Both arms plus the chunk vector arm; the LightRAG default.
    Mix,
}

/// Query parameters with the reference default values (`base.py:10-29`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryParam {
    pub mode: QueryMode,
    pub only_need_context: bool,
    pub response_type: String,
    pub level: i64,
    pub top_k: usize,
    pub naive_max_token_for_text_unit: usize,
    pub local_max_token_for_text_unit: usize,
    pub local_max_token_for_local_context: usize,
    pub local_max_token_for_community_report: usize,
    pub local_community_single_one: bool,
    pub global_min_community_rating: f64,
    pub global_max_consider_community: usize,
    pub global_max_token_for_community_report: usize,
    /// LightRAG query budgets (`base.py:93-167`); the reference defaults are
    /// the server-side values from `constants.py:57-61`.
    pub chunk_top_k: usize,
    pub max_entity_tokens: usize,
    pub max_relation_tokens: usize,
    pub max_total_tokens: usize,
    /// Cosine threshold shared by both vector arms (`base.py:338`).
    pub cosine_better_than_threshold: f32,
    /// Rerank switch; LightRAG defaults to true (`base.py:166`), we default
    /// off (offline-first deviation, stage doc §7).
    pub enable_rerank: bool,
    pub min_rerank_score: f64,
}

impl Default for QueryParam {
    fn default() -> Self {
        Self {
            mode: QueryMode::Global,
            only_need_context: false,
            response_type: "Multiple Paragraphs".to_string(),
            level: 2,
            top_k: 20,
            naive_max_token_for_text_unit: 12_000,
            local_max_token_for_text_unit: 4_000,
            local_max_token_for_local_context: 4_800,
            local_max_token_for_community_report: 3_200,
            local_community_single_one: false,
            global_min_community_rating: 0.0,
            global_max_consider_community: 512,
            global_max_token_for_community_report: 16_384,
            chunk_top_k: DEFAULT_CHUNK_TOP_K,
            max_entity_tokens: DEFAULT_MAX_ENTITY_TOKENS,
            max_relation_tokens: DEFAULT_MAX_RELATION_TOKENS,
            max_total_tokens: DEFAULT_MAX_TOTAL_TOKENS,
            cosine_better_than_threshold: DEFAULT_COSINE_THRESHOLD,
            enable_rerank: false,
            min_rerank_score: 0.0,
        }
    }
}

/// OpenAI-style chat message; used for the reference-parity cache key
/// (`compute_args_hash`) and by the LLM client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}
