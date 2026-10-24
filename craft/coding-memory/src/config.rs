//! Configuration for the coding craft's store.
//!
//! The two settings that reach the engine: which embedder to use, and the
//! `created_at` baseline. They mirror the memory craft's configuration, so
//! one model can serve both crafts and the timestamps agree.

use serde::{Deserialize, Serialize};

/// How the coding craft's store is configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Local embedding model (a `fastembed` model name). Code and prose share
    /// the default here so one model serves both crafts.
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Baseline for a fact's `created_at` (a millisecond timestamp).
    ///
    /// A fixed baseline rather than the current time: writing the same turn
    /// twice must not make the second write look like a correction just
    /// because the clock moved.
    #[serde(default = "default_created_at_base")]
    pub created_at_base: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            embedding_model: default_embedding_model(),
            created_at_base: default_created_at_base(),
        }
    }
}

fn default_embedding_model() -> String {
    "BAAI/bge-m3".to_string()
}

fn default_created_at_base() -> i64 {
    0
}
