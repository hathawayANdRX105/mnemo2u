//! `mnemo2u` — memory/RAG library for coding harnesses.
//!
//! This crate is the **facade**: the published surface of the library. The
//! implementation lives in [`mnemo_core`]; this crate re-exports it under the
//! module paths the library has always published, so `mnemo2u::core::traits::*`
//! and friends keep resolving for every downstream consumer.
//!
//! Three-store design: turso (truth: facts, audit, bi-temporal, BM25) +
//! redb (graph: adjacency) + lancedb (vector ANN). The truth layer is
//! transactional; the graph and vector indexes are derived and rebuildable.
//!
//! Module map (all of it re-exported from `mnemo-core`):
//!
//! - [`core`]  — types, text/parity utilities, storage traits (nano-graphrag `base.py`)
//! - [`store`] — backend implementations (turso / lancedb + in-memory)
//! - [`llm`]   — LLM + embedding client adapters
//! - [`graph`] — graph pipeline: chunking, extraction, merge, communities (R1)
//! - [`index`] — write path: incremental merge, purge, cost gates (R2)
//! - [`query`] — read path: arms → fusion → rerank (R3)
//! - [`diag`]  — read-path diagnosis: miss attribution + arm health (R7)
//! - [`tune`]  — tuning surface: knob registry + scoreboard baseline (R7 S2)

pub use mnemo_core::core;
pub use mnemo_core::diag;
pub use mnemo_core::eval;
pub use mnemo_core::graph;
pub use mnemo_core::index;
pub use mnemo_core::llm;
pub use mnemo_core::query;
pub use mnemo_core::store;
pub use mnemo_core::tune;

pub use mnemo_core::assemble;
pub use mnemo_core::config;
pub use mnemo_core::pipeline;

pub use mnemo_core::core::rag::{
    Chunk, CommunitySchema, DocRecord, EntityRecord, QueryMode, QueryParam, RelationRecord,
};
pub use mnemo_core::core::text::{compute_mdhash_id, Tokenizer, GRAPH_FIELD_SEP};
pub use mnemo_core::core::traits::{
    Embedder, GraphSnapshot, GraphStore, KvStore, Result as StoreResult, StoreError, VectorHit,
    VectorRow, VectorStore,
};
pub use mnemo_core::core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
