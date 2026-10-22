//! Golden parity: our chunker vs the reference implementation.
//!
//! Fixture produced by `tools/golden/gen_chunks.py` (verbatim copy of
//! `refs/nano-graphrag/nano_graphrag/_op.py` chunking + tiktoken).

use mnemo2u::graph::chunk::{
    get_chunks, DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE, DEFAULT_CHUNK_TOKEN_SIZE,
};
use mnemo2u::Tokenizer;
use serde_json::Value;

fn fixture() -> Value {
    let raw = std::fs::read_to_string("tests/fixtures/chunk_golden.json").expect("fixture exists");
    serde_json::from_str(&raw).expect("fixture is valid json")
}

#[test]
fn chunking_matches_python_reference() {
    let fixture = fixture();
    let overlap = fixture["config"]["overlap_token_size"]
        .as_u64()
        .expect("overlap") as usize;
    let max = fixture["config"]["max_token_size"].as_u64().expect("max") as usize;
    assert_eq!(
        (overlap, max),
        (DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE, DEFAULT_CHUNK_TOKEN_SIZE),
        "fixture must use the effective reference defaults"
    );

    let docs: Vec<(String, String)> = fixture["docs"]
        .as_object()
        .expect("docs object")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value["content"].as_str().expect("doc content").to_string(),
            )
        })
        .collect();
    let expected = fixture["chunks"].as_object().expect("chunks object");

    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    let ours = get_chunks(&docs, &tokenizer, overlap, max).expect("chunking runs");

    assert_eq!(
        ours.len(),
        expected.len(),
        "chunk count differs from the reference"
    );
    for (id, chunk) in &ours {
        let want = expected
            .get(id)
            .unwrap_or_else(|| panic!("reference has no chunk {id}"));
        assert_eq!(
            chunk.tokens,
            want["tokens"].as_u64().expect("tokens") as usize,
            "tokens of {id}"
        );
        assert_eq!(
            chunk.chunk_order_index,
            want["chunk_order_index"].as_u64().expect("order") as usize,
            "chunk_order_index of {id}"
        );
        assert_eq!(
            chunk.full_doc_id,
            want["full_doc_id"].as_str().expect("doc id"),
            "doc of {id}"
        );
        assert_eq!(
            chunk.content,
            want["content"].as_str().expect("content"),
            "content of {id}"
        );
    }
}

#[test]
fn chunk_ids_are_pure_functions_of_content() {
    let fixture = fixture();
    let docs: Vec<(String, String)> = fixture["docs"]
        .as_object()
        .expect("docs object")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value["content"].as_str().expect("doc content").to_string(),
            )
        })
        .collect();
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");

    let first = get_chunks(
        &docs,
        &tokenizer,
        DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE,
        DEFAULT_CHUNK_TOKEN_SIZE,
    )
    .expect("first run");
    let second = get_chunks(
        &docs,
        &tokenizer,
        DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE,
        DEFAULT_CHUNK_TOKEN_SIZE,
    )
    .expect("second run");

    let ids = |chunks: &[(String, mnemo2u::Chunk)]| -> Vec<String> {
        chunks.iter().map(|(id, _)| id.clone()).collect()
    };
    assert_eq!(
        ids(&first),
        ids(&second),
        "re-inserting the same text must reuse chunk ids"
    );
}
