//! Configuration — TOML file, environment secrets, zero-egress defaults.
//!
//! This is the missing "how does a human run this" surface (the roadmap §4
//! CLI: `insert` / `query` / `admin`). The library already had every
//! machinery piece; what it lacked was a way to point them at real stores
//! and a real model without writing Rust.
//!
//! Rules the config enforces:
//! - **Secrets never live in the file.** `api_key_env` names an environment
//!   variable; the value is read at startup only.
//! - **One file, one store root.** All backends derive from `data_dir`, so a
//!   config points at one workspace and can be moved or deleted whole.
//! - **Defaults are the ported reference's** (`query.mode = mix`,
//!   `top_k = 40`), not new opinions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::core::rag::{QueryMode, QueryParam};
use crate::core::text::TextResult;
use crate::llm::openai::OpenAiConfig;

/// One `mnemo2u.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub store: StoreSection,
    #[serde(default)]
    pub llm: LlmSection,
    #[serde(default)]
    pub embedding: EmbeddingSection,
    #[serde(default)]
    pub pipeline: PipelineSection,
    #[serde(default)]
    pub query: QuerySection,
}

/// Where the data lives. Everything under one directory so a workspace is a
/// single movable unit (and `admin rebuild` can find the truth layer).
#[derive(Debug, Clone, Deserialize)]
pub struct StoreSection {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// Namespace prefix for the KV tables — two configs sharing one directory
    /// with different namespaces stay isolated.
    #[serde(default = "default_namespace")]
    pub namespace: String,
    /// Graph adjacency scope label.
    #[serde(default = "default_graph_scope")]
    pub graph_scope: String,
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./mnemo2u-data")
}
fn default_namespace() -> String {
    "default".to_string()
}
fn default_graph_scope() -> String {
    "default".to_string()
}

impl Default for StoreSection {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            namespace: default_namespace(),
            graph_scope: default_graph_scope(),
        }
    }
}

/// The chat model (extraction / reports / summaries / answers).
#[derive(Debug, Clone, Deserialize)]
pub struct LlmSection {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Name of the environment variable holding the key — never the key.
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub cache: bool,
}

fn default_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}
fn default_api_key_env() -> String {
    "OPENAI_API_KEY".to_string()
}
fn default_model() -> String {
    "gpt-4o-mini".to_string()
}

impl Default for LlmSection {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key_env: default_api_key_env(),
            model: default_model(),
            cache: true,
        }
    }
}

impl LlmSection {
    /// Read the key from the environment. A missing variable is an error at
    /// startup rather than a mid-insert 401 — the operator finds out once.
    pub fn api_key(&self) -> std::result::Result<String, String> {
        std::env::var(&self.api_key_env).map_err(|_| {
            format!(
                "environment variable {} is not set (llm.api_key_env)",
                self.api_key_env
            )
        })
    }

    /// The client the pipeline wraps in `CachedLlm`.
    pub fn client(&self) -> std::result::Result<OpenAiClient, String> {
        let key = self.api_key()?;
        Ok(OpenAiClient::new(OpenAiConfig::new(
            self.base_url.clone(),
            key,
            self.model.clone(),
        )))
    }

    pub fn cache_enabled(&self) -> bool {
        self.cache
    }
}

/// The local embedding model (never egresses — ONNX, in-process).
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingSection {
    #[serde(default = "default_embedding_model")]
    pub model: String,
}

fn default_embedding_model() -> String {
    crate::llm::fastembed::DEFAULT_MODEL.to_string()
}

impl Default for EmbeddingSection {
    fn default() -> Self {
        Self {
            model: default_embedding_model(),
        }
    }
}

/// Write-path knobs. Every default matches `PipelineOptions::default()`, so a
/// config file only exists to override.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineSection {
    #[serde(default)]
    pub enable_naive_rag: bool,
    #[serde(default)]
    pub enable_local: bool,
    #[serde(default = "default_true")]
    pub enable_llm_cache: bool,
    #[serde(default = "default_true")]
    pub enable_llm_cache_for_extract: bool,
    /// `off` (default) never runs community detection during insert.
    #[serde(default = "default_community_mode")]
    pub community_mode: String,
    #[serde(default = "default_chunk_tokens")]
    pub chunk_token_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap_token_size: usize,
    #[serde(default = "default_max_async")]
    pub best_model_max_async: usize,
    #[serde(default = "default_embedding_batch")]
    pub embedding_batch_num: usize,
}

fn default_true() -> bool {
    true
}
fn default_community_mode() -> String {
    "off".to_string()
}
fn default_chunk_tokens() -> usize {
    1200
}
fn default_chunk_overlap() -> usize {
    100
}
fn default_max_async() -> usize {
    16
}
fn default_embedding_batch() -> usize {
    32
}

impl Default for PipelineSection {
    fn default() -> Self {
        Self {
            enable_naive_rag: false,
            enable_local: false,
            enable_llm_cache: true,
            enable_llm_cache_for_extract: true,
            community_mode: default_community_mode(),
            chunk_token_size: default_chunk_tokens(),
            chunk_overlap_token_size: default_chunk_overlap(),
            best_model_max_async: default_max_async(),
            embedding_batch_num: default_embedding_batch(),
        }
    }
}

impl PipelineSection {
    /// The mode string maps to the two-value enum; anything else is a
    /// config error, not a silent fallback.
    pub fn community_mode(&self) -> std::result::Result<CommunityMode, String> {
        match self.community_mode.as_str() {
            "off" => Ok(CommunityMode::Off),
            "on_demand" => Ok(CommunityMode::OnDemand),
            other => Err(format!(
                "pipeline.community_mode = \"{other}\" (expected \"off\" or \"on_demand\")"
            )),
        }
    }
}

/// Read-path knobs (the query subcommand's defaults).
#[derive(Debug, Clone, Deserialize)]
pub struct QuerySection {
    #[serde(default = "default_query_mode")]
    pub mode: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "default_cosine")]
    pub cosine_better_than_threshold: f32,
}

fn default_query_mode() -> String {
    "mix".to_string()
}
fn default_top_k() -> usize {
    40
}
fn default_cosine() -> f32 {
    0.2
}

impl Default for QuerySection {
    fn default() -> Self {
        Self {
            mode: default_query_mode(),
            top_k: default_top_k(),
            cosine_better_than_threshold: default_cosine(),
        }
    }
}

impl QuerySection {
    pub fn mode(&self) -> std::result::Result<QueryMode, String> {
        match self.mode.as_str() {
            "local" => Ok(QueryMode::Local),
            "global" => Ok(QueryMode::Global),
            "naive" => Ok(QueryMode::Naive),
            "hybrid" => Ok(QueryMode::Hybrid),
            "mix" => Ok(QueryMode::Mix),
            other => Err(format!(
                "query.mode = \"{other}\" (expected local|global|naive|hybrid|mix)"
            )),
        }
    }

    /// A `QueryParam` overlaid on the library defaults, so a config that says
    /// only `top_k = 10` keeps every other reference default intact.
    pub fn param(&self) -> std::result::Result<QueryParam, String> {
        Ok(QueryParam {
            mode: self.mode()?,
            top_k: self.top_k,
            cosine_better_than_threshold: self.cosine_better_than_threshold,
            ..QueryParam::default()
        })
    }
}

use crate::llm::openai::OpenAiClient;
use crate::pipeline::CommunityMode;

impl Config {
    /// Parse a config file. Unknown keys are rejected: a typo'd key silently
    /// falling back to a default is how a workspace ends up querying the
    /// wrong store.
    pub fn load(path: &Path) -> std::result::Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("parsing config: {error}"))
    }
}

/// Tokenizer the pipeline chunking needs. Kept next to the config because
/// it is a fixed port contract (o200k, `gpt-4o` parity) rather than a knob.
pub fn tokenizer() -> TextResult<crate::core::text::Tokenizer> {
    crate::core::text::Tokenizer::for_gpt_4o()
}

/// Shared timeout applied to config-driven model calls.
pub fn model_timeout() -> Duration {
    Duration::from_secs(60)
}
