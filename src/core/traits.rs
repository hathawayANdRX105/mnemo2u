//! Storage trait interfaces, one per store in the three-store design.
//!
//! Mirrors nano-graphrag's pluggable `*_storage_cls` interfaces so any backend
//! can be swapped behind a trait without touching pipeline code:
//! - `GraphStore`  -> kuzu (Cypher, multi-hop, PageRank/Leiden hooks)
//! - `VectorStore` -> lancedb (ANN over fact/entity embeddings)
//! - `FactStore`   -> turso (truth layer: facts, audit, bi-temporal, FTS BM25)

use async_trait::async_trait;
use std::collections::HashMap;

use crate::core::types::{Edge, Fact, FactId, RelKind, ScopeKey};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("scope violation: {0}")]
    ScopeViolation(String),
    #[error("stale revision for {0}")]
    StaleRevision(String),
}

/// Result alias with defaulted error type (repo rule).
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Graph store: edges and traversal. Fact bodies are NOT stored here.
#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn upsert_edges(&self, edges: &[Edge]) -> Result<()>;
    async fn remove_edges_from(&self, src: &FactId) -> Result<()>;
    /// Bounded n-hop neighborhood from a set of seed fact ids.
    async fn neighbors(&self, seeds: &[FactId], hops: u8, scope: &ScopeKey) -> Result<Vec<Edge>>;
    async fn edges_by_rel(&self, scope: &ScopeKey, rel: RelKind) -> Result<Vec<Edge>>;
}

/// Vector store: ANN similarity over embeddings keyed by (scope, model, id).
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(
        &self,
        scope: &ScopeKey,
        model: &str,
        id: &FactId,
        vector: &[f32],
    ) -> Result<()>;
    async fn remove(&self, scope: &ScopeKey, id: &FactId) -> Result<()>;
    /// Hard scope filter must apply inside the search, not after it.
    async fn search(
        &self,
        scope: &ScopeKey,
        model: &str,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<(FactId, f32)>>;
}

/// Fact store: the single transactional truth layer.
#[async_trait]
pub trait FactStore: Send + Sync {
    async fn put(&self, fact: &Fact) -> Result<()>;
    async fn get(&self, scope: &ScopeKey, id: &FactId) -> Result<Option<Fact>>;
    async fn supersede(&self, old: &FactId, new: &Fact) -> Result<()>;
    async fn search(
        &self,
        scope: &ScopeKey,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(FactId, f64)>>;
    async fn append_audit(&self, op: &str, payload_hash: &str) -> Result<()>;
}

/// Lexical (BM25 via Turso Tantivy FTS index) lives behind FactStore::search
/// in the truth layer; this trait exists only if an alternative lexical
/// backend is ever needed. Enabled by config, never silently double-backed.
#[async_trait]
pub trait LexicalStore: Send + Sync {
    async fn index(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()>;
    async fn drop(&self, scope: &ScopeKey, id: &FactId) -> Result<()>;
    async fn search(
        &self,
        scope: &ScopeKey,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<(FactId, f64)>>;
}

/// Local embedding backend (e.g. fastembed ONNX bge-small). Offline after
/// first model download.
#[async_trait]
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> String;
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// Reciprocal-rank fusion over per-arm rankings (atlas: RRF k=60 default,
/// a default not a measurement — tune on your own corpus before defending it).
pub fn rrf_fuse(rankings: &[Vec<FactId>], k: u32) -> Vec<FactId> {
    let mut scores: HashMap<FactId, f64> = HashMap::new();
    for ranking in rankings {
        for (i, id) in ranking.iter().enumerate() {
            *scores.entry(id.clone()).or_default() += 1.0 / (k as f64 + i as f64 + 1.0);
        }
    }
    let mut fused: Vec<(FactId, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused.into_iter().map(|(id, _)| id).collect()
}
