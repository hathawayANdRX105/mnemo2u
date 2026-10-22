//! Chunk fusion — `process_chunks_unified` (`utils.py:7055`).
//!
//! After the search arms produce their raw chunk lists, the reference runs
//! one pipeline: dedup → optional rerank → score filter → `chunk_top_k` →
//! token budget. Rerank is off by default in this fork (offline-first
//! deviation, stage doc §7), so the seam exists but no rerank model ships.

use crate::core::rag::QueryParam;
use crate::core::text::truncate_list_by_token_size;
use crate::core::text::Tokenizer;
use crate::query::context::ChunkRow;

/// Rerank hook. The reference calls a model here; this fork keeps the seam
/// typed so enabling it later is one implementation, not a refactor.
pub trait Reranker: Send + Sync {
    /// Returns a relevance score per chunk, in input order.
    fn score(&self, query: &str, chunks: &[ChunkRow]) -> Vec<f32>;
}

/// Dedup by chunk id, keeping first-seen order (`process_chunks_unified`
/// step 0 — the caller passes already-deduped rows, this is the guard).
pub fn dedup_chunks(rows: Vec<ChunkRow>) -> Vec<ChunkRow> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for row in rows {
        if !seen.contains(&row.id) {
            seen.push(row.id.clone());
            out.push(row);
        }
    }
    out
}

/// The fusion pipeline. Returns the rows that fit, after every cap.
pub fn fuse_chunks(
    rows: Vec<ChunkRow>,
    query: &str,
    param: &QueryParam,
    tokenizer: &Tokenizer,
    reranker: Option<&dyn Reranker>,
) -> Vec<ChunkRow> {
    let mut unique = dedup_chunks(rows);

    if param.enable_rerank {
        if let Some(reranker) = reranker {
            let scores = reranker.score(query, &unique);
            let mut indexed: Vec<(ChunkRow, f32)> = unique.into_iter().zip(scores).collect();
            // Descending score; ties keep first-seen order (stable sort).
            indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            unique = indexed
                .into_iter()
                .filter(|(_, score)| f64::from(*score) >= param.min_rerank_score)
                .map(|(row, _)| row)
                .collect();
        }
    }

    // chunk_top_k, then the token budget over the chunk contents.
    unique.truncate(param.chunk_top_k);
    truncate_list_by_token_size(
        &unique,
        |row| row.content.clone(),
        param.max_total_tokens,
        tokenizer,
    )
}
