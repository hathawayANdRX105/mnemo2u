//! mnemo2u — memory/RAG library for coding harnesses.
//!
//! Three-store design: turso (truth: facts, audit, bi-temporal, BM25) +
//! kuzu (graph: edges, multi-hop, community) + lancedb (vector ANN).
//! Single crate; big features live in subdirectories:
//!
//! - [`core`]  — types and storage traits (nano-graphrag `base.py` mapping)
//! - [`store`] — backend implementations (turso / kuzu / lancedb)
//! - [`llm`]   — LLM + embedding client adapters
//! - [`graph`] — graph pipeline: extraction, merge, communities (R1)
//! - [`index`] — write path: slice → candidates → gateway (R2)
//! - [`query`] — read path: arms → fusion → rerank (R3)

pub mod core;
pub mod graph;
pub mod index;
pub mod llm;
pub mod query;
pub mod store;

pub use core::traits::{Embedder, FactStore, GraphStore, LexicalStore, VectorStore};
pub use core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
