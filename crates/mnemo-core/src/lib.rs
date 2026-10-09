//! mnemo2u core: shared types and storage traits.
//!
//! Shape follows nano-graphrag's `base.py` component interfaces, mapped onto
//! the three-store design: graph (kuzu) + vector (lancedb) + facts/lexical
//! (turso). The original transcript always lives in the harness SessionDb;
//! every record here carries a `SourceRef` back to it (evidence-before-belief).

pub mod traits;
pub mod types;

pub use traits::{Embedder, FactStore, GraphStore, LexicalStore, VectorStore};
pub use types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
