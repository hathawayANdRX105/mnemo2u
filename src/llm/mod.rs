//! LLM + embedding adapters.
//!
//! R1 (nano-graphrag port) needs a text LLM for entity/relation extraction and
//! community reports: OpenAI-compatible HTTP client behind an `LlmClient`
//! trait, with a deterministic mock for tests (no network in CI).
//! Embeddings: `fastembed` (ONNX, local; model id feeds the vector cache key).
//!
//! NOT YET IMPLEMENTED: R1.
