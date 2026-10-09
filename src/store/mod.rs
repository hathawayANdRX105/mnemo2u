//! Store: backend implementations for the three-store design.
//!
//! - `turso`   -> FactStore + LexicalStore (truth layer; Tantivy BM25 FTS,
//!               `fts_match`/`fts_score` — NOT SQLite FTS5 syntax)
//! - `kuzu`    -> GraphStore (Cypher traversal; upstream archived, pinned
//!               frozen release + trait isolation)
//! - `lancedb` -> VectorStore (ANN)
//!
//! NOT YET IMPLEMENTED: backends land in R1 (turso+lancedb first, kuzu after).
//! No backend may be registered as available before its contract tests pass.
