//! Store backends.

pub mod memory;
pub mod turso;

// NOT YET IMPLEMENTED: turso / kuzu / lancedb (R1.T6, R1.T10).
// No backend may be registered as available before its contract tests pass.
