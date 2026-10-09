//! mnemo-query: read path (scoped three-arm recall + fusion + L0/L1 tiers).
//!
//! Order: hard scope filter inside every arm → vector (lancedb) + lexical
//! (turso Tantivy BM25) + graph neighborhood (kuzu) → `rrf_fuse` →
//! optional Jev rerank → `exists_verdict` decides hit vs no_match.
//! Returns SourceRef + L0 abstract first; caller `read`s L2 original text.
//! Arm failure must be distinguishable from an arm returning empty.
//!
//! NOT YET IMPLEMENTED: R3.
