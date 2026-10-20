//! Store backends.

pub mod facts;
pub mod lance_facts;
pub mod lancedb;
pub mod memory;
pub mod redb_graph;
pub mod repair;
pub mod turso;
pub mod turso_edges;
pub mod turso_facts;
pub mod turso_lexical;

// `memory` covers the pipeline for tests; `turso` (truth), `redb_graph` (the
// derived graph index) and `lancedb` are the durable backends. A backend is
// only registered as available once its contract tests pass.
