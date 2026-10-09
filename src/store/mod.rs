//! Store backends.

#[cfg(feature = "kuzu-backend")]
pub mod kuzu;
pub mod lancedb;
pub mod memory;
pub mod turso;

// `memory` covers the pipeline for tests; `lancedb` and `kuzu` (behind
// `kuzu-backend`) are the durable backends. A backend is only registered as
// available once its contract tests pass.
