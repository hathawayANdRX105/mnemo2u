//! Core: shared types, text utilities and storage trait interfaces.

pub mod concurrency;
pub mod rag;
pub mod text;
pub mod traits;
pub mod types;

pub use concurrency::Limiter;
pub use rag::{
    ChatMessage, Chunk, CommunitySchema, DocRecord, EntityRecord, QueryMode, QueryParam,
    RelationRecord,
};
pub use text::{compute_args_hash, compute_mdhash_id, Tokenizer};
pub use traits::{
    Embedder, FactStore, GraphSnapshot, GraphStore, KvStore, LexicalStore, Result, StoreError,
    VectorHit, VectorRow, VectorStore,
};
pub use types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
