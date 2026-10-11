//! Store backends.

pub mod facts;
pub mod lance_facts;
pub mod lancedb;
pub mod memory;
pub mod repair;
pub mod turso;
pub mod turso_edges;
pub mod turso_graph;
pub mod turso_lexical;

// `memory` covers the pipeline for tests; `turso`/`turso_graph`/`lancedb` are the
// durable backends. A backend is only registered as available once its contract
// tests pass.
