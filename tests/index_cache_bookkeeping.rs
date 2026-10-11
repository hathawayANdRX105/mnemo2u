//! `llm_cache_list` bookkeeping — the chunk rows record the LLM cache rows
//! their extraction triggered, and a document delete drops exactly those rows.
//!
//! Reference: `use_llm_func_with_cache` collects the keys through
//! `cache_keys_collector` (`utils.py:5617-5643`), `adelete_by_doc_id` reads
//! them back off the chunk rows and deletes the rows (:7078-7103, :7175-7200).

use std::sync::Arc;

use mnemo2u::core::text::compute_mdhash_id;
use mnemo2u::core::traits::KvStore;
use mnemo2u::index::merge::IndexMergeOptions;
use mnemo2u::index::purge::{delete_document, PurgeStores};
use mnemo2u::index::tracking::TrackingStores;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::{MockEmbedder, RoutedLlm};
use mnemo2u::pipeline::{CommunityMode, InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::memory::{MemoryGraph, MemoryKv, MemoryVector};
use mnemo2u::store::repair::RepairQueue;
use mnemo2u::Tokenizer;
use serde_json::Value;

/// The extraction prompt contains this phrase; the gleaning round does not,
/// so one rule per stage keeps the routing deterministic.
const EXTRACTION: &str = "entity<|#|>ACME<|#|>organization<|#|>Acme makes things.\n\
relation<|#|>ACME<|#|>BETA LABS<|#|>ACME acquired Beta Labs.<|#|>partnership\n\
<|COMPLETE|>";
const GLEAN: &str = "entity<|#|>GAMMA<|#|>organization<|#|>Gamma ships software.";

struct Fixture {
    pipeline: Pipeline,
    cache: Arc<MemoryKv>,
    llm: Arc<RoutedLlm>,
}

fn fixture() -> Fixture {
    let llm = Arc::new(RoutedLlm::new(
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
    let cache = Arc::new(MemoryKv::new());
    let repair = Arc::new(RepairQueue::new(Arc::new(MemoryKv::new())));
    let embedder = Arc::new(MockEmbedder::new());

    let pipeline = Pipeline {
        full_docs: Arc::new(MemoryKv::new()),
        text_chunks: Arc::new(MemoryKv::new()),
        community_reports: Arc::new(MemoryKv::new()),
        graph: Arc::new(MemoryGraph::new()),
        entities_vdb: Arc::new(MemoryVector::new(embedder.clone(), 0.2)),
        relationships_vdb: Some(Arc::new(MemoryVector::new(embedder.clone(), 0.2))
            as Arc<dyn mnemo2u::core::traits::VectorStore>),
        tracking: TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        full_entities: Arc::new(MemoryKv::new()),
        full_relations: Arc::new(MemoryKv::new()),
        chunks_vdb: None,
        llm: CachedLlm::new(llm.clone(), cache.clone()),
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        repair,
        options: PipelineOptions {
            community_mode: CommunityMode::Off,
            ..PipelineOptions::default()
        },
    };
    Fixture {
        pipeline,
        cache,
        llm,
    }
}

/// The `llm_cache_list` recorded on a chunk row, in the order written.
fn recorded_cache_keys(row: &Value) -> Vec<String> {
    row.get("llm_cache_list")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn inserted_chunk_rows(pipeline: &Pipeline) -> Vec<(String, Value)> {
    let keys = pipeline.text_chunks.all_keys().await.expect("chunk keys");
    let mut rows: Vec<(String, Value)> = Vec::new();
    for key in keys {
        let row = pipeline
            .text_chunks
            .get_by_id(&key)
            .await
            .expect("row read")
            .expect("chunk row");
        rows.push((key, row));
    }
    rows
}

#[tokio::test]
async fn chunk_rows_record_the_cache_keys_extraction_triggered() {
    let fixture = fixture();
    let outcome = fixture
        .pipeline
        .insert(vec!["ACME acquired Beta Labs in 2024.".to_string()])
        .await
        .expect("insert");
    assert!(matches!(outcome, InsertOutcome::Inserted { chunks: 1, .. }));

    let rows = inserted_chunk_rows(&fixture.pipeline).await;
    assert_eq!(rows.len(), 1);
    let keys = recorded_cache_keys(&rows[0].1);
    assert!(
        !keys.is_empty(),
        "the chunk row must record the cache rows its extraction triggered"
    );
    // Deduped: the list must not name the same cache row twice.
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        keys.len(),
        "no duplicate keys on one chunk row"
    );
    for key in &keys {
        assert!(key.starts_with("default:extract:"), "extract-namespace key");
        assert!(
            fixture
                .cache
                .get_by_id(key)
                .await
                .expect("cache read")
                .is_some(),
            "recorded key {key} points at a live cache row"
        );
    }
}

#[tokio::test]
async fn reinserting_the_same_document_does_not_grow_the_cache_lists() {
    let fixture = fixture();
    let content = "ACME acquired Beta Labs in 2024.".to_string();
    assert!(matches!(
        fixture
            .pipeline
            .insert(vec![content.clone()])
            .await
            .expect("first"),
        InsertOutcome::Inserted { .. }
    ));
    let before = inserted_chunk_rows(&fixture.pipeline).await;
    let cache_rows_before = fixture.cache.all_keys().await.expect("cache keys").len();

    // The replayed document is known: nothing re-runs, nothing grows.
    assert_eq!(
        fixture
            .pipeline
            .insert(vec![content])
            .await
            .expect("second"),
        InsertOutcome::AllDocsKnown
    );
    let after = inserted_chunk_rows(&fixture.pipeline).await;
    assert_eq!(before.len(), after.len());
    for ((_, before_row), (_, after_row)) in before.iter().zip(&after) {
        assert_eq!(
            recorded_cache_keys(before_row),
            recorded_cache_keys(after_row),
            "the list must not grow on a replay"
        );
    }
    assert_eq!(
        fixture.cache.all_keys().await.expect("cache keys").len(),
        cache_rows_before,
        "a replayed document must not add cache rows"
    );
}

#[tokio::test]
async fn deleting_the_document_drops_its_cached_extract_rows() {
    let fixture = fixture();
    let content = "ACME acquired Beta Labs in 2024.".to_string();
    fixture
        .pipeline
        .insert(vec![content.clone()])
        .await
        .expect("insert");
    let doc_id = compute_mdhash_id(&content, "doc-");

    let recorded: Vec<String> = inserted_chunk_rows(&fixture.pipeline)
        .await
        .into_iter()
        .flat_map(|(_, row)| recorded_cache_keys(&row))
        .collect();
    assert!(!recorded.is_empty(), "capture the recorded rows first");
    for key in &recorded {
        assert!(
            fixture
                .cache
                .get_by_id(key)
                .await
                .expect("cache read")
                .is_some(),
            "recorded key {key} is live before the delete"
        );
    }

    let report = delete_document(
        &doc_id,
        &PurgeStores {
            graph: &*fixture.pipeline.graph,
            entities_vdb: &*fixture.pipeline.entities_vdb,
            relationships_vdb: fixture.pipeline.relationships_vdb.as_deref(),
            text_chunks: &*fixture.pipeline.text_chunks,
            full_docs: &*fixture.pipeline.full_docs,
            full_entities: &*fixture.pipeline.full_entities,
            full_relations: &*fixture.pipeline.full_relations,
            llm_cache: &*fixture.cache,
            tracking: &fixture.pipeline.tracking,
            llm: &fixture.pipeline.llm,
            tokenizer: &fixture.pipeline.tokenizer,
            options: &IndexMergeOptions::default(),
        },
    )
    .await
    .expect("purge");
    assert_eq!(report.deleted_chunks, 1, "the document's chunk went");

    // The deletion assertion: every recorded key is gone from the cache KV.
    for key in &recorded {
        assert!(
            fixture
                .cache
                .get_by_id(key)
                .await
                .expect("cache read")
                .is_none(),
            "cache row {key} must be dropped with the document"
        );
    }

    // And behaviorally: re-inserting the same content is a cache miss again,
    // so the extraction calls the model instead of a deleted row.
    let calls_before = fixture.llm.calls();
    assert!(matches!(
        fixture
            .pipeline
            .insert(vec![content])
            .await
            .expect("re-insert"),
        InsertOutcome::Inserted { .. }
    ));
    assert!(
        fixture.llm.calls() > calls_before,
        "with the cache rows gone, extraction must call the model again"
    );
}
