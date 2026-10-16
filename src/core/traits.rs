//! Storage and embedding trait interfaces — one per store in the three-store
//! design, mirroring nano-graphrag's pluggable surface (`base.py`) so pipeline
//! code never names a backend:
//!
//! - [`GraphStore`]  -> turso adjacency / in-memory (tests)
//! - [`VectorStore`] -> lancedb / in-memory (tests)
//! - [`KvStore`]     -> turso / in-memory or JSON files (tests)
//! - [`Embedder`]    -> fastembed (local ONNX) / deterministic mock (tests)
//!
//! `FactStore`/`LexicalStore`/`FactVectorStore`/`FactEdgeStore` describe the
//! memory layer (R3); the read-path arms implement them, the R1 pipeline does
//! not consume them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::collections::HashMap;

use async_trait::async_trait;

use crate::core::types::{Edge, Fact, FactId, RelKind, ScopeKey};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("scope violation: {0}")]
    ScopeViolation(String),
    #[error("stale revision: {0}")]
    StaleRevision(String),
    #[error("not found: {0}")]
    NotFound(String),
}

/// Result alias with defaulted error type (repo rule).
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// One vector row: `content` is embedded by the store's embedder, `meta`
/// rides along (reference `BaseVectorStorage.upsert`, base.py:85-89).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRow {
    pub id: String,
    pub content: String,
    pub meta: Value,
}

/// One ANN hit (`NanoVectorDBStorage.query` maps `id`/`distance`/meta).
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub id: String,
    pub distance: f32,
    pub meta: Value,
}

/// Full graph state; clustering recomputes globally over this snapshot
/// (`gdb_networkx.py::_leiden_clustering`).
#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<(String, Value)>,
    pub edges: Vec<(String, String, Value)>,
}

/// Key-value truth storage (`BaseKVStorage`, base.py:93-113).
#[async_trait]
pub trait KvStore: Send + Sync {
    async fn all_keys(&self) -> Result<Vec<String>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Value>>;
    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<Option<Value>>>;
    /// Keys that are **not yet present** (reference `filter_keys` returns
    /// un-exist keys, base.py:105-107).
    async fn filter_keys(&self, ids: &[String]) -> Result<Vec<String>>;
    async fn upsert(&self, rows: Vec<(String, Value)>) -> Result<()>;
    /// Drop rows by key; unknown keys are ignored. Needed by the repair queue
    /// (replayed items leave) and tombstone paths (R2).
    async fn remove(&self, keys: &[String]) -> Result<()>;
    async fn drop_all(&self) -> Result<()>;
    async fn index_done(&self) -> Result<()>;
}

/// Vector storage (`BaseVectorStorage`, base.py:78-89). The embedder is held
/// by the backend, so callers pass text.
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(&self, rows: Vec<VectorRow>) -> Result<()>;
    /// Cosine-thresholded ANN query (`NanoVectorDBStorage.query`,
    /// vdb_nanovectordb.py:53-64; threshold default 0.2).
    async fn query(&self, query: &str, top_k: usize) -> Result<Vec<VectorHit>>;
    /// Drop rows by id; unknown ids are ignored. Needed by the cascade delete
    /// path (R2): a deleted document's entity/relation/chunk vectors and the
    /// reverse-direction relation ids go away with it.
    async fn remove(&self, ids: &[String]) -> Result<()>;
    /// Persist pending writes (reference `index_done_callback`).
    async fn index_done(&self) -> Result<()>;
}

/// Graph storage (`BaseGraphStorage`, base.py:117-186).
#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn has_node(&self, node_id: &str) -> Result<bool>;
    async fn has_edge(&self, src: &str, tgt: &str) -> Result<bool>;
    async fn get_node(&self, node_id: &str) -> Result<Option<Value>>;
    async fn get_nodes_batch(&self, ids: &[String]) -> Result<Vec<Option<Value>>>;
    async fn get_edge(&self, src: &str, tgt: &str) -> Result<Option<Value>>;
    async fn get_edges_batch(&self, pairs: &[(String, String)]) -> Result<Vec<Option<Value>>>;
    async fn node_degree(&self, node_id: &str) -> Result<i64>;
    async fn node_degrees_batch(&self, ids: &[String]) -> Result<Vec<i64>>;
    async fn edge_degree(&self, src: &str, tgt: &str) -> Result<i64>;
    async fn edge_degrees_batch(&self, pairs: &[(String, String)]) -> Result<Vec<i64>>;
    async fn node_edges(&self, node_id: &str) -> Result<Option<Vec<(String, String)>>>;
    async fn nodes_edges_batch(&self, ids: &[String])
        -> Result<Vec<Option<Vec<(String, String)>>>>;
    async fn upsert_node(&self, node_id: &str, data: Value) -> Result<()>;
    async fn upsert_nodes_batch(&self, rows: Vec<(String, Value)>) -> Result<()>;
    async fn upsert_edge(&self, src: &str, tgt: &str, data: Value) -> Result<()>;
    async fn upsert_edges_batch(&self, rows: Vec<(String, String, Value)>) -> Result<()>;
    /// Drop a node and every edge that touches it (cascade delete, R2).
    async fn remove_node(&self, node_id: &str) -> Result<()>;
    /// Drop one directed edge (cascade delete, R2).
    async fn remove_edge(&self, src: &str, tgt: &str) -> Result<()>;
    /// Everything, for clustering / rebuild paths.
    async fn snapshot(&self) -> Result<GraphSnapshot>;
    async fn index_done(&self) -> Result<()>;
}

/// Local embedding backend (fastembed ONNX bge-small in production; a
/// deterministic mock in tests). Reference `EmbeddingFunc`, `_utils.py:266`.
#[async_trait]
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> String;
    fn dim(&self) -> usize;
    fn max_token_size(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Fact store: the single transactional truth layer (R3; the memory layer's
/// only authoritative store — every recall arm re-derives from its rows).
#[async_trait]
pub trait FactStore: Send + Sync {
    async fn put(&self, fact: &Fact) -> Result<()>;
    async fn get(&self, scope: &ScopeKey, id: &FactId) -> Result<Option<Fact>>;
    /// Mark `old` superseded by `new` (the bi-temporal correction path): the
    /// old row keeps its history but leaves `search` results.
    async fn supersede(&self, old: &FactId, new: &Fact) -> Result<()>;
    /// Scope pre-filtered recall: the predicate applies *inside* the store,
    /// never as a post-filter over unscoped ranks (stage doc 03 contract 1).
    /// Superseded rows never appear. Scoring is backend-defined.
    async fn search(
        &self,
        scope: &ScopeKey,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(FactId, f64)>>;
    /// Every row the scope admits — including superseded and `valid_to`-closed
    /// ones (the truth-surface view that maintenance paths need, R5.T4), in
    /// stable id order. Callers filter with [`Fact::is_current`] themselves.
    async fn scan(&self, scope: &ScopeKey) -> Result<Vec<Fact>>;
    async fn append_audit(&self, op: &str, payload_hash: &str) -> Result<()>;
}

/// Lexical backends other than the Turso Tantivy FTS index (R3+; enabled by
/// config, never silently double-backed).
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

/// Vector arm over fact embeddings (stage doc 03 T2). The embedding cache key
/// is `hash(scope, model, text)` — scope is part of the key so an identical
/// text never ships a vector across scopes.
#[async_trait]
pub trait FactVectorStore: Send + Sync {
    async fn upsert(&self, scope: &ScopeKey, id: &FactId, embedding: &[f32]) -> Result<()>;
    async fn remove(&self, scope: &ScopeKey, id: &FactId) -> Result<()>;
    /// ANN top-`limit` for `query`. The scope predicate applies *inside* the
    /// vector search (stage doc 03 contract 1), never as a post-filter over
    /// unscoped top-k.
    async fn search(
        &self,
        scope: &ScopeKey,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<(FactId, f64)>>;
}

/// Fact-graph edge access for the memory-layer graph arm (stage doc 03 T4).
/// Kept separate from [`GraphStore`] (the RAG entity graph). Every row
/// carries the writer's scope, so traversal filters *inside* the store —
/// an edge is only ever visible to the scope its facts live in.
#[async_trait]
pub trait FactEdgeStore: Send + Sync {
    async fn upsert_edges(&self, scope: &ScopeKey, edges: &[Edge]) -> Result<()>;
    async fn remove_edges_from(&self, scope: &ScopeKey, src: &FactId) -> Result<()>;
    /// Bounded n-hop neighborhood from `seeds`; the scope predicate applies
    /// inside every hop's query.
    async fn neighbors(&self, seeds: &[FactId], hops: u8, scope: &ScopeKey) -> Result<Vec<Edge>>;
    async fn edges_by_rel(&self, scope: &ScopeKey, rel: RelKind) -> Result<Vec<Edge>>;
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
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0 .0.cmp(&b.0 .0))
    });
    fused.into_iter().map(|(id, _)| id).collect()
}
