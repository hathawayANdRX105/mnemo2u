//! Chunking — port of `_op.py::chunking_by_token_size` (31-58) and
//! `get_chunks` (94-108).

use std::collections::HashMap;

use crate::core::rag::Chunk;
use crate::core::text::{compute_mdhash_id, TextResult, Tokenizer};

/// Effective defaults: the `GraphRAG` dataclass overrides the function
/// defaults (graphrag.py:76-79 vs `_op.py:36-37`).
pub const DEFAULT_CHUNK_TOKEN_SIZE: usize = 1200;
pub const DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE: usize = 100;

/// Token-window chunking with overlap. Per-document `chunk_order_index`
/// restarts at 0 (reference `for i, chunk in enumerate(chunk_texts)`).
pub fn chunking_by_token_size(
    tokens_list: &[Vec<u32>],
    doc_keys: &[String],
    doc_paths: &[String],
    tok: &Tokenizer,
    overlap_token_size: usize,
    max_token_size: usize,
) -> TextResult<Vec<Chunk>> {
    assert!(
        overlap_token_size < max_token_size,
        "overlap must be smaller than the window (reference range() would not advance)"
    );
    let step = max_token_size - overlap_token_size;
    let mut results = Vec::new();
    for (index, tokens) in tokens_list.iter().enumerate() {
        let mut chunk_tokens: Vec<Vec<u32>> = Vec::new();
        let mut lengths: Vec<usize> = Vec::new();
        let mut start = 0usize;
        while start < tokens.len() {
            let end = (start + max_token_size).min(tokens.len());
            chunk_tokens.push(tokens[start..end].to_vec());
            lengths.push(max_token_size.min(tokens.len() - start));
            start += step;
        }
        let chunk_texts = tok.decode_batch(&chunk_tokens)?;
        for (i, chunk) in chunk_texts.iter().enumerate() {
            results.push(Chunk {
                tokens: lengths[i],
                content: chunk.trim().to_string(),
                chunk_order_index: i,
                full_doc_id: doc_keys[index].clone(),
                file_path: doc_paths[index].clone(),
            });
        }
    }
    Ok(results)
}

/// `get_chunks` — encode docs, chunk them, key each chunk by `chunk-<md5>`.
///
/// Python fills a dict with `update()`, so duplicate content keeps the first
/// position and the last value; same here.
pub fn get_chunks(
    new_docs: &[(String, String)],
    tok: &Tokenizer,
    overlap_token_size: usize,
    max_token_size: usize,
) -> TextResult<Vec<(String, Chunk)>> {
    let tokens: Vec<Vec<u32>> = new_docs
        .iter()
        .map(|(_, content)| tok.encode(content))
        .collect();
    let doc_keys: Vec<String> = new_docs.iter().map(|(key, _)| key.clone()).collect();
    let doc_paths: Vec<String> = new_docs.iter().map(|(_, path)| path.clone()).collect();
    let chunks = chunking_by_token_size(
        &tokens,
        &doc_keys,
        &doc_paths,
        tok,
        overlap_token_size,
        max_token_size,
    )?;

    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, Chunk> = HashMap::new();
    for chunk in chunks {
        let id = compute_mdhash_id(&chunk.content, "chunk-");
        if !by_id.contains_key(&id) {
            order.push(id.clone());
        }
        by_id.insert(id, chunk);
    }
    Ok(order
        .into_iter()
        .map(|id| {
            let chunk = by_id.remove(&id).expect("id came from the same map");
            (id, chunk)
        })
        .collect())
}
