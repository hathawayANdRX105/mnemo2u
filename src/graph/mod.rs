//! Graph pipeline (nano-graphrag core, R1):
//! chunking -> entity/relation extraction -> merge/dedup -> graph build ->
//! community detection (leiden-rs) -> community reports.
//!
//! Ported from nano-graphrag `_op.py`; storage goes to the three-store layer,
//! not the reference's json/nano-vectordb/networkx.
//!
//! Implemented so far: [`chunk`] (R1.T2).

pub mod chunk;
pub mod community;
pub mod extract;
pub mod merge;
pub mod prompts;
pub mod reports;

pub use chunk::{
    chunking_by_token_size, get_chunks, DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE, DEFAULT_CHUNK_TOKEN_SIZE,
};
pub use extract::{extract_entities, parse_extraction_result, ExtractOptions, ExtractedRecords};
pub use merge::{merge_edges_then_upsert, merge_nodes_then_upsert, MergeOptions};
