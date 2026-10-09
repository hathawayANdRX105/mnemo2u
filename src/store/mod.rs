//! Store backends.

pub mod lancedb;
pub mod memory;
pub mod turso;
pub mod turso_graph;

// `memory` covers the pipeline for tests; `turso`/`turso_graph`/`lancedb` are the
// durable backends. A backend is only registered as available once its contract
// tests pass.
