//! Extraction loop behaviour with a scripted LLM
//! (`lightrag/operate.py::extract_entities`, initial + one gleaning round).
//!
//! The reference runs exactly one gleaning round when `max_gleaning > 0` and
//! skips it when system + history + continue prompt exceed
//! `MAX_EXTRACT_INPUT_TOKENS` (`operate.py:4252-4286`). There is no yes/no
//! loop-decision prompt: that was the nano-graphrag flow this port replaced.

use std::sync::Arc;

use mnemo2u::core::rag::Chunk;
use mnemo2u::core::text::Tokenizer;
use mnemo2u::graph::extract::{extract_entities, ExtractOptions};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockLlm;
use mnemo2u::store::memory::MemoryKv;

fn chunk(content: &str) -> (String, Chunk) {
    (
        "chunk-1".to_string(),
        Chunk {
            tokens: 10,
            content: content.to_string(),
            chunk_order_index: 0,
            full_doc_id: "doc-1".to_string(),
            file_path: "doc-1".to_string(),
        },
    )
}

// Real extractions end with the completion delimiter (the prompt requires it).
// The record layout is LightRAG's: no parentheses, `<|#|>` field separator,
// entities carry 4 fields, relations carry 5 (no strength column).
const INITIAL: &str = "entity<|#|>\"ACME\"<|>\"organization\"<|>\"Acme makes things.\"\n\
                       relation<|#|>\"ACME\"<|>\"BETA\"<|>\"owns\"<|>\"Acme owns Beta.\"<|COMPLETE|>";
const GLEAN: &str = "entity<|#|>\"GAMMA\"<|>\"organization\"<|>\"Gamma labs.\"<|COMPLETE|>";

#[tokio::test]
async fn gleaning_runs_exactly_one_round() {
    let chunks = vec![chunk("ACME acquired Beta Labs.")];
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec![INITIAL.to_string(), GLEAN.to_string()],
    ));
    let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));
    let options = ExtractOptions {
        max_gleaning: 1,
        best_model_max_async: 4,
        max_extract_input_tokens: 20_480,
    };
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");

    let records = extract_entities(&chunks, &llm, &options, &tokenizer)
        .await
        .expect("extraction");

    assert_eq!(records.nodes.len(), 2, "initial + gleaning entities merge");
    assert_eq!(records.edges.len(), 1, "the initial relation is kept");
    assert_eq!(
        inner.calls(),
        2,
        "max_gleaning=1 means exactly one continue call, no loop decision"
    );
    let prompts = inner.sent_prompts();
    assert!(
        prompts[0].contains("ACME acquired Beta Labs."),
        "the initial user prompt embeds the chunk text"
    );
    assert!(
        prompts[1].contains("continue"),
        "the gleaning call sends the continue prompt"
    );
    // The relation keywords come from tuple field 3.
    let edge = &records.edges[0].1[0];
    assert_eq!(edge.keywords, "owns");
    assert_eq!(edge.weight, 1.0, "LightRAG tuples carry no strength field");
}

#[tokio::test]
async fn gleaning_skipped_over_the_input_budget() {
    let chunks = vec![chunk("ACME acquired Beta Labs.")];
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec![INITIAL.to_string(), GLEAN.to_string()],
    ));
    let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));
    let options = ExtractOptions {
        max_gleaning: 1,
        best_model_max_async: 4,
        // The system prompt alone is larger than this, so the precheck skips
        // the gleaning call entirely (`operate.py:4252-4286`).
        max_extract_input_tokens: 1,
    };
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");

    let records = extract_entities(&chunks, &llm, &options, &tokenizer)
        .await
        .expect("extraction");

    assert_eq!(
        records.nodes.len(),
        1,
        "only the initial extraction is used"
    );
    assert_eq!(
        inner.calls(),
        1,
        "over-budget gleaning must not reach the model"
    );
}

#[tokio::test]
async fn identical_chunks_hit_the_cache_across_runs() {
    let chunks = vec![chunk("ACME acquired Beta Labs.")];
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec![INITIAL.to_string(), GLEAN.to_string()],
    ));
    let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));
    let options = ExtractOptions {
        max_gleaning: 1,
        best_model_max_async: 4,
        max_extract_input_tokens: 20_480,
    };
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");

    extract_entities(&chunks, &llm, &options, &tokenizer)
        .await
        .expect("first run");
    let after_first = inner.calls();
    extract_entities(&chunks, &llm, &options, &tokenizer)
        .await
        .expect("second run");

    assert_eq!(after_first, 2);
    assert_eq!(
        inner.calls(),
        2,
        "re-extracting the same chunk must come from the cache"
    );
}
