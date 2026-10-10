//! Read path (R2): LightRAG's unified keyword-arm search — `local`/`global`
//! are the single-arm cases of `hybrid`/`mix`, `naive` keeps the chunk-vector
//! arm. R1's community-report query arms (`local_query`, `global_query`) are
//! superseded: LightRAG never implemented community detection, and its global
//! arm reads `relationships_vdb` instead.
//!
//! Two orthogonal dimensions stay separate (stage doc §0): the keyword arms
//! decide *which* records are fetched, the fusion/context stages decide *how
//! much* of them fits the budget.

pub mod context;
pub mod fusion;
pub mod keywords;
pub mod kg;
pub mod naive;

use std::sync::Arc;

use crate::core::rag::{QueryMode, QueryParam};
use crate::core::text::Tokenizer;
use crate::core::traits::{GraphStore, KvStore, VectorStore};
use crate::llm::cache::CachedLlm;

/// LightRAG's cosine cut-off for both vector arms (`base.py:338`).
pub use crate::core::rag::DEFAULT_COSINE_THRESHOLD;

/// Everything the query paths read from.
#[derive(Clone)]
pub struct QueryStores {
    pub graph: Arc<dyn GraphStore>,
    pub entities_vdb: Arc<dyn VectorStore>,
    /// `relationships_vdb` — the global/high-level arm; absent backends make
    /// the hl arm silently skip, like the reference's optional store.
    pub relationships_vdb: Option<Arc<dyn VectorStore>>,
    /// `chunks_vdb` exists only when naive RAG is enabled (`graphrag.py:214-222`).
    pub chunks_vdb: Option<Arc<dyn VectorStore>>,
    pub text_chunks: Arc<dyn KvStore>,
    pub community_reports: Arc<dyn KvStore>,
    pub llm: CachedLlm,
    pub tokenizer: Tokenizer,
    /// `best_model_max_async` (`graphrag.py:119`).
    pub max_async: usize,
}

impl QueryStores {
    /// Dispatch by mode (`kg_query` mode table, operate.py:5366-5461; `naive`
    /// keeps its own entry point, lightrag.py:5056).
    pub async fn query(&self, query: &str, param: &QueryParam) -> crate::llm::LlmResult<String> {
        match param.mode {
            QueryMode::Naive => naive::naive_query(self, query, param).await,
            QueryMode::Local | QueryMode::Global | QueryMode::Hybrid | QueryMode::Mix => {
                // `Ok(None)` is the reference's "no context could be built";
                // the facade surfaces its fail_response text instead.
                Ok(kg::kg_query(self, query, param)
                    .await?
                    .unwrap_or_else(|| crate::graph::prompts::FAIL_RESPONSE.to_string()))
            }
        }
    }
}
