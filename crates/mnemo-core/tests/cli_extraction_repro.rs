//! Focused reproduction: does extraction work when driven through a
//! content-routed mock, exactly as the CLI does?

use std::sync::Arc;

use mnemo2u::core::rag::{Chunk, EntityRecord, RelationRecord};
use mnemo2u::core::text::Tokenizer;
use mnemo2u::graph::extract::{extract_entities, ExtractOptions};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::RoutedLlm;
use mnemo2u::store::memory::MemoryKv;

const EXTRACTION: &str = "entity<|#|>mnemo2u<|#|>project<|#|>mnemo2u is a memory and RAG library for coding harnesses.\nrelation<|#|>mnemo2u<|#|>Rust<|#|>mnemo2u is written in Rust.<|#|>built_with\n<|COMPLETE|>";
const GLEAN: &str = "entity<|#|>turso<|#|>technology<|#|>turso is the truth layer of mnemo2u.";

fn chunk(key: &str, content: &str) -> (String, Chunk) {
    (
        key.to_string(),
        Chunk {
            tokens: 0,
            content: content.to_string(),
            chunk_order_index: 0,
            full_doc_id: "doc-1".to_string(),
            file_path: "doc.md".to_string(),
        },
    )
}

#[tokio::test]
async fn extraction_yields_records_through_a_routed_mock() {
    let routed = Arc::new(RoutedLlm::new(
        "mock-best",
        vec![
            (
                "Extract entities and relationships".to_string(),
                EXTRACTION.to_string(),
            ),
            (
                "missed or incorrectly formatted".to_string(),
                GLEAN.to_string(),
            ),
        ],
    ));
    let llm = CachedLlm::new(routed, Arc::new(MemoryKv::new()));
    let records: Vec<(String, Chunk)> = vec![chunk(
        "chunk-1",
        "mnemo2u is a Rust memory library using turso and lancedb.",
    )];
    let extracted = extract_entities(
        &records,
        &llm,
        &ExtractOptions::default(),
        &Tokenizer::for_gpt_4o().expect("tokenizer"),
    )
    .await
    .expect("extraction");

    assert!(
        !extracted.nodes.is_empty(),
        "entities should be extracted from the routed mock reply"
    );
    assert!(
        !extracted.edges.is_empty(),
        "relations should be extracted from the routed mock reply"
    );

    // Prove the entity/relation bodies really parsed, so a silent format
    // drift cannot make this test pass vacuously.
    let names: Vec<&str> = extracted
        .nodes
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(names.contains(&"mnemo2u"), "names: {names:?}");
    let (_, node) = extracted
        .nodes
        .iter()
        .find(|(name, _)| name == "mnemo2u")
        .expect("mnemo2u node");
    assert!(
        node.iter()
            .any(|record: &EntityRecord| record.description.contains("RAG")),
        "descriptions: {:?}",
        node.iter()
            .map(|r| r.description.clone())
            .collect::<Vec<_>>()
    );
    // The pair key is stored alphabetically sorted, so assert on both sides.
    let pairs: Vec<(String, String)> = extracted
        .edges
        .iter()
        .map(|(pair, _)| pair.clone())
        .collect();
    assert!(
        pairs
            .iter()
            .any(|(src, tgt)| src == "mnemo2u" || tgt == "mnemo2u"),
        "edges: {pairs:?}"
    );
    let (_, edge_records) = extracted.edges.first().expect("an edge record");
    let edge: &RelationRecord = edge_records.first().expect("an edge record entry");
    assert!(!edge.description.is_empty(), "edge description parsed");
}
