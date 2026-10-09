//! Extraction loop behaviour with a scripted LLM
//! (`_op.py::_process_single_content`, gleaning + if-loop).

use std::sync::Arc;

use mnemo2u::core::rag::Chunk;
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
        },
    )
}

// Real extractions end with the completion delimiter (the prompt requires it),
// and gleaning results are appended verbatim — the mock must do the same or
// record splitting merges both responses into one.
const INITIAL: &str = "(\"entity\"<|>\"ACME\"<|>\"ORG\"<|>\"Acme makes things.\")<|COMPLETE|>";
const GLEAN: &str = "(\"entity\"<|>\"BETA\"<|>\"ORG\"<|>\"Beta labs.\")<|COMPLETE|>";

#[tokio::test]
async fn gleaning_runs_with_default_single_pass() {
    let chunks = vec![chunk("ACME acquired Beta Labs.")];
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec![INITIAL.to_string(), GLEAN.to_string()],
    ));
    let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));
    let options = ExtractOptions {
        max_gleaning: 1,
        best_model_max_async: 4,
    };

    let records = extract_entities(&chunks, &llm, &options)
        .await
        .expect("extraction");

    assert_eq!(records.nodes.len(), 2, "initial + gleaning records merge");
    assert_eq!(
        inner.calls(),
        2,
        "max_gleaning=1 means one continue call and no if-loop"
    );
    let prompts = inner.sent_prompts();
    assert!(
        prompts[0].contains("ACME acquired Beta Labs."),
        "initial prompt must embed the chunk text"
    );
    assert!(
        prompts[1].contains("missed"),
        "the continue prompt is sent for gleaning"
    );
}

#[tokio::test]
async fn if_loop_answer_stops_gleaning() {
    let chunks = vec![chunk("ACME acquired Beta Labs.")];
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec![INITIAL.to_string(), GLEAN.to_string(), "No".to_string()],
    ));
    let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));
    let options = ExtractOptions {
        max_gleaning: 3,
        best_model_max_async: 4,
    };

    let records = extract_entities(&chunks, &llm, &options)
        .await
        .expect("extraction");

    assert_eq!(records.nodes.len(), 2);
    assert_eq!(
        inner.calls(),
        3,
        "a non-\"yes\" if-loop answer stops further gleaning"
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
    };

    extract_entities(&chunks, &llm, &options)
        .await
        .expect("first run");
    let after_first = inner.calls();
    extract_entities(&chunks, &llm, &options)
        .await
        .expect("second run");

    assert_eq!(after_first, 2);
    assert_eq!(
        inner.calls(),
        2,
        "re-extracting the same chunk must come from the cache"
    );
}
