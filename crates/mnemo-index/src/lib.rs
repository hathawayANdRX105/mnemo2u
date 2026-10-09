//! mnemo-index: write path (zero-LLM capture → governed write gateway).
//!
//! Pipeline shape follows nano-graphrag `_op.py`:
//! chunk closed turns → extract candidates (regex/local, no LLM) →
//! judge dedupe/conflict (Jev, behind opt-in egress gate) → single
//! transactional commit across FactStore + GraphStore + VectorStore +
//! LexicalStore. Watermark `organized_through` commits with the result.
//!
//! NOT YET IMPLEMENTED: R2.
