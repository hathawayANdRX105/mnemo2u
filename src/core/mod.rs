//! Core: shared types and storage trait interfaces.

pub mod traits;
pub mod types;

pub use traits::{Embedder, FactStore, GraphStore, LexicalStore, VectorStore};
pub use types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
