//! mnemo2u — memory/RAG library for coding harnesses.
//!
//! Three-store design: turso (truth: facts, audit, bi-temporal, BM25) +
//! kuzu (graph: entities, edges, communities) + lancedb (vector ANN).
//! Single crate; big features live in subdirectories:
//!
//! - [`core`]  — types, text/parity utilities, storage traits (nano-graphrag `base.py`)
//! - [`store`] — backend implementations (turso / kuzu / lancedb + in-memory)
//! - [`llm`]   — LLM + embedding client adapters
//! - [`graph`] — graph pipeline: chunking, extraction, merge, communities (R1)
//! - [`index`] — write path: incremental merge, purge, cost gates (R2)
//! - [`query`] — read path: arms → fusion → rerank (R3)

pub mod core;
pub mod graph;
pub mod index;
pub mod llm;
pub mod pipeline;
pub mod query;
pub mod store;

pub use core::rag::{
    Chunk, CommunitySchema, DocRecord, EntityRecord, QueryMode, QueryParam, RelationRecord,
};
pub use core::text::{compute_mdhash_id, Tokenizer, GRAPH_FIELD_SEP};
pub use core::traits::{
    Embedder, GraphSnapshot, GraphStore, KvStore, Result as StoreResult, StoreError, VectorHit,
    VectorRow, VectorStore,
};
pub use core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
