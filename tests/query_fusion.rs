//! Chunk fusion — dedup, caps and the rerank seam (`src/query/fusion.rs`).

use mnemo2u::core::rag::QueryParam;
use mnemo2u::core::text::Tokenizer;
use mnemo2u::query::context::ChunkRow;
use mnemo2u::query::fusion::{dedup_chunks, fuse_chunks, Reranker};

fn row(id: &str, content: &str) -> ChunkRow {
    ChunkRow {
        id: id.to_string(),
        content: content.to_string(),
        file_path: "doc-1".to_string(),
        chunk_order_index: 0,
    }
}

fn param() -> QueryParam {
    QueryParam {
        chunk_top_k: 10,
        max_total_tokens: 100,
        enable_rerank: false,
        min_rerank_score: 0.0,
        ..QueryParam::default()
    }
}

/// A reranker that inverts relevance: the second chunk is "best".
struct InvertedReranker;

impl Reranker for InvertedReranker {
    fn score(&self, _query: &str, chunks: &[ChunkRow]) -> Vec<f32> {
        chunks
            .iter()
            .enumerate()
            .map(|(index, _)| (index + 1) as f32)
            .collect()
    }
}

#[test]
fn dedup_keeps_first_seen_order() {
    let rows = vec![row("DC1", "a"), row("DC2", "b"), row("DC1", "a again")];
    let deduped = dedup_chunks(rows);
    assert_eq!(
        deduped
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["DC1", "DC2"]
    );
    assert_eq!(deduped[0].content, "a");
}

#[test]
fn chunk_top_k_caps_the_result() {
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    let rows = vec![row("DC1", "a"), row("DC2", "b"), row("DC3", "c")];
    let param = QueryParam {
        chunk_top_k: 2,
        ..param()
    };
    let fused = fuse_chunks(rows, "query", &param, &tokenizer, None);
    assert_eq!(fused.len(), 2);
    assert_eq!(fused[0].id, "DC1");
    assert_eq!(fused[1].id, "DC2");
}

#[test]
fn total_token_budget_caps_the_result() {
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    // `truncate_list_by_token_size` charges `tokens + 1` per row: two-word rows
    // cost 3 each, so a 6-token budget keeps two of three.
    let rows = vec![
        row("DC1", "alpha beta"),
        row("DC2", "gamma delta"),
        row("DC3", "epsilon zeta"),
    ];
    let param = QueryParam {
        chunk_top_k: 10,
        max_total_tokens: 6,
        ..param()
    };
    let fused = fuse_chunks(rows, "query", &param, &tokenizer, None);
    assert_eq!(fused.len(), 2, "the token budget trims the tail");
}

#[test]
fn zero_token_budget_returns_nothing() {
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    let rows = vec![row("DC1", "alpha beta")];
    let param = QueryParam {
        max_total_tokens: 0,
        ..param()
    };
    assert!(fuse_chunks(rows, "query", &param, &tokenizer, None).is_empty());
}

#[test]
fn rerank_disabled_skips_the_seam() {
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    let rows = vec![row("DC1", "a"), row("DC2", "b")];
    let param = param();
    let fused = fuse_chunks(rows, "query", &param, &tokenizer, Some(&InvertedReranker));
    assert_eq!(fused[0].id, "DC1", "rerank off keeps retrieval order");
}

#[test]
fn rerank_enabled_reorders_and_filters() {
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    let rows = vec![row("DC1", "a"), row("DC2", "b")];
    // The inverted reranker scores DC1=1, DC2=2.
    let kept = QueryParam {
        enable_rerank: true,
        min_rerank_score: 0.5,
        ..param()
    };
    let fused = fuse_chunks(
        rows.clone(),
        "query",
        &kept,
        &tokenizer,
        Some(&InvertedReranker),
    );
    assert_eq!(fused.len(), 2, "both rows clear the 0.5 floor");
    assert_eq!(fused[0].id, "DC2", "the higher score moves first");

    let filtered = QueryParam {
        enable_rerank: true,
        min_rerank_score: 1.5,
        ..param()
    };
    let fused = fuse_chunks(
        rows,
        "query",
        &filtered,
        &tokenizer,
        Some(&InvertedReranker),
    );
    assert_eq!(fused.len(), 1, "scores 1 and 2: only 2 passes the floor");
    assert_eq!(fused[0].id, "DC2", "the survivor is the best-scoring row");
}
