//! Configuration for the tavern craft's store.
//!
//! Two settings reach the engine: which embedder, and the `created_at`
//! baseline. They mirror the other crafts' configuration so one model serves
//! the whole repo and the timestamps agree.

use serde::{Deserialize, Serialize};

/// How the tavern craft's store is configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Local embedding model (a `fastembed` model name).
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Baseline for a fact's `created_at` (a millisecond timestamp).
    ///
    /// A fixed baseline rather than the current time: re-writing the same
    /// session must not make an unchanged fact look like a correction just
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
