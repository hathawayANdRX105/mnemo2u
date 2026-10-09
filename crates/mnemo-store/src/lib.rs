//! mnemo-store: three-store implementations behind mnemo-core traits.
//!
//! - turso  -> FactStore + LexicalStore (truth layer, Tantivy BM25 FTS index)
//! - kuzu   -> GraphStore (Cypher traversal; pinned frozen release)
//! - lancedb -> VectorStore (ANN)
//!
//! NOT YET IMPLEMENTED: backends land after the R1 contract tests exist.
