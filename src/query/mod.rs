//! Read path (R1): the reference's three query modes — `local_query`,
//! `global_query`, `naive_query` (`_op.py:700-1140`).
//!
//! Scope-prefiltered multi-arm recall, RRF fusion and Jev rerank land in R3;
//! this module is the faithful port of the reference behaviour.

pub mod global;
pub mod local;
pub mod naive;

use std::sync::Arc;

use crate::core::rag::QueryParam;
use crate::core::text::Tokenizer;
use crate::core::traits::{GraphStore, KvStore, VectorStore};
use crate::llm::cache::CachedLlm;

/// Everything the query paths read from.
#[derive(Clone)]
pub struct QueryStores {
    pub graph: Arc<dyn GraphStore>,
    pub entities_vdb: Arc<dyn VectorStore>,
    /// `chunks_vdb` exists only when naive RAG is enabled (`graphrag.py:214-222`).
    pub chunks_vdb: Option<Arc<dyn VectorStore>>,
    pub community_reports: Arc<dyn KvStore>,
    pub text_chunks: Arc<dyn KvStore>,
    pub llm: CachedLlm,
    pub tokenizer: Tokenizer,
    /// `best_model_max_async` (`graphrag.py:119`).
    pub max_async: usize,
}

impl QueryStores {
    /// Dispatch by mode (`GraphRAG.aquery`, graphrag.py:236-275).
    pub async fn query(&self, query: &str, param: &QueryParam) -> crate::llm::LlmResult<String> {
        match param.mode {
            crate::core::rag::QueryMode::Local => local::local_query(self, query, param).await,
            crate::core::rag::QueryMode::Global => global::global_query(self, query, param).await,
            crate::core::rag::QueryMode::Naive => naive::naive_query(self, query, param).await,
        }
    }
}
