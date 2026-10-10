//! Write path (R2): LightRAG's incremental merge over the R1 commit protocol —
//! chunk tracking rows, per-document merge scopes, cascade delete, and the
//! cost/session gates the stage doc defers to a later slice.
//!
//! Sharing rule with R1: derived stores (graph, vectors, tracking) are written
//! during the run with idempotent upserts, the truth rows (`full_docs`,
//! `text_chunks`) commit last, and a failed derived write goes to the repair
//! queue. Nothing here opens a second truth layer.

pub mod cost;
pub mod ingest;
pub mod merge;
pub mod purge;
pub mod tracking;

pub use cost::{ChunkValueScorer, ChunkVerdict, CostLedger, CostOptions, CostStage};
pub use ingest::{OrganizedThrough, SessionIngest, SessionTurn, Turn};
pub use tracking::{
    apply_source_ids_limit, make_relation_chunk_key, merge_source_ids, parse_relation_chunk_key,
    TrackingStores, LIMIT_FIFO, LIMIT_KEEP,
};
