//! Cascade delete — the delete-outright vs rebuild classification and the
//! store removals (`src/index/purge.rs`).

use std::sync::Arc;

use mnemo2u::core::rag::{EntityRecord, RelationRecord};
use mnemo2u::core::text::{compute_mdhash_id, Tokenizer};
use mnemo2u::core::traits::{GraphStore, KvStore, VectorStore};
use mnemo2u::index::merge::{IndexMergeOptions, ScopeAnchor};
use mnemo2u::index::purge::{delete_document, PurgeStores};
use mnemo2u::index::tracking::TrackingStores;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockLlm;
use mnemo2u::store::memory::{MemoryGraph, MemoryKv, MemoryVector};
use serde_json::json;

/// Embedder + stores for one purge scenario.
struct Fixture {
    graph: MemoryGraph,
    entities_vdb: MemoryVector,
    relationships_vdb: MemoryVector,
    text_chunks: MemoryKv,
    full_docs: MemoryKv,
    full_entities: MemoryKv,
    full_relations: MemoryKv,
    llm_cache: MemoryKv,
    tracking: TrackingStores,
    llm: CachedLlm,
    tokenizer: Tokenizer,
}

fn fixture() -> Fixture {
    let embedder = Arc::new(mnemo2u::llm::mock::MockEmbedder::new());
    Fixture {
        graph: MemoryGraph::new(),
        entities_vdb: MemoryVector::new(embedder.clone(), 0.0),
        relationships_vdb: MemoryVector::new(embedder, 0.0),
        text_chunks: MemoryKv::new(),
        full_docs: MemoryKv::new(),
        full_entities: MemoryKv::new(),
        full_relations: MemoryKv::new(),
        llm_cache: MemoryKv::new(),
        tracking: TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        llm: CachedLlm::new(
            Arc::new(MockLlm::new("mock", vec!["SUMMARY".to_string()])),
            Arc::new(MemoryKv::new()),
        ),
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
    }
}

/// Two documents each contributing to ACME, and BETA owned by only one of them.
async fn seed_two_documents(fixture: &Fixture) {
    let options = IndexMergeOptions::default();
    let shared = EntityRecord {
        entity_name: "ACME".to_string(),
        entity_type: "organization".to_string(),
        description: "makes things".to_string(),
        source_id: "chunk-a".to_string(),
        file_path: "docs/a.md".to_string(),
        timestamp: 0,
    };
    let exclusive = EntityRecord {
        entity_name: "BETA".to_string(),
        entity_type: "organization".to_string(),
        description: "labs".to_string(),
        source_id: "chunk-a".to_string(),
        file_path: "docs/a.md".to_string(),
        timestamp: 0,
    };
    let edge = RelationRecord {
        src_id: "ACME".to_string(),
        tgt_id: "BETA".to_string(),
        weight: 1.0,
        description: "owns".to_string(),
        source_id: "chunk-a".to_string(),
        order: 1,
        keywords: "owns".to_string(),
        file_path: "docs/a.md".to_string(),
        timestamp: 0,
    };
    let acme = mnemo2u::index::merge::merge_node(
        "ACME",
        &[shared],
        &fixture.graph,
        &fixture.llm,
        &fixture.tokenizer,
        &options,
    )
    .await
    .expect("merge node");
    mnemo2u::index::merge::merge_node(
        "BETA",
        &[exclusive],
        &fixture.graph,
        &fixture.llm,
        &fixture.tokenizer,
        &options,
    )
    .await
    .expect("merge node");
    let edge_row = mnemo2u::index::merge::merge_edge(
        "ACME",
        "BETA",
        &[edge],
        &fixture.graph,
        &fixture.llm,
        &fixture.tokenizer,
        &options,
    )
    .await
    .expect("merge edge");

    let entity_rows = mnemo2u::index::merge::entity_vector_rows(std::slice::from_ref(&acme));
    fixture
        .entities_vdb
        .upsert(entity_rows)
        .await
        .expect("entity vectors");
    let relation_rows =
        mnemo2u::index::merge::relation_vector_rows("ACME", "BETA", &edge_row, "chunk-a");
    fixture
        .relationships_vdb
        .upsert(relation_rows)
        .await
        .expect("relation vectors");

    // Truth rows: doc-1's chunks plus a second document's chunk that also owns
    // ACME, so the delete of doc-1 must rebuild rather than drop it.
    fixture
        .full_docs
        .upsert(vec![
            ("doc-1".to_string(), json!({"content": "a"})),
            ("doc-2".to_string(), json!({"content": "b"})),
        ])
        .await
        .expect("docs");
    fixture
        .text_chunks
        .upsert(vec![(
            "chunk-a".to_string(),
            json!({"content": "a", "full_doc_id": "doc-1", "file_path": "docs/a.md"}),
        )])
        .await
        .expect("chunks");
    fixture
        .tracking
        .entity_chunks
        .upsert(vec![
            (
                "ACME".to_string(),
                json!({"chunks": ["chunk-a", "chunk-b"]}),
            ),
            ("BETA".to_string(), json!({"chunks": ["chunk-a"]})),
        ])
        .await
        .expect("entity tracking");
    fixture
        .tracking
        .relation_chunks
        .upsert(vec![(
            "ACME<SEP>BETA".to_string(),
            json!({"chunks": ["chunk-a"]}),
        )])
        .await
        .expect("relation tracking");
    let anchor = ScopeAnchor {
        entities: vec!["ACME".to_string(), "BETA".to_string()],
        relations: vec!["ACME<SEP>BETA".to_string()],
    };
    fixture
        .full_entities
        .upsert(vec![(
            "doc-1".to_string(),
            json!({"entities": anchor.entities}),
        )])
        .await
        .expect("anchor");
    fixture
        .full_relations
        .upsert(vec![(
            "doc-1".to_string(),
            json!({"relations": anchor.relations}),
        )])
        .await
        .expect("anchor");
    fixture
        .llm_cache
        .upsert(vec![(
            "default:extract:hash-1".to_string(),
            json!({"return": "x", "chunk_ids": ["chunk-a"]}),
        )])
        .await
        .expect("cache row");
}

#[tokio::test]
async fn shared_entity_is_rebuilt_and_exclusive_one_is_deleted() {
    let fixture = fixture();
    seed_two_documents(&fixture).await;

    let report = delete_document(
        "doc-1",
        &PurgeStores {
            graph: &fixture.graph,
            entities_vdb: &fixture.entities_vdb,
            relationships_vdb: Some(&fixture.relationships_vdb),
            text_chunks: &fixture.text_chunks,
            full_docs: &fixture.full_docs,
            full_entities: &fixture.full_entities,
            full_relations: &fixture.full_relations,
            llm_cache: &fixture.llm_cache,
            tracking: &fixture.tracking,
            llm: &fixture.llm,
            tokenizer: &fixture.tokenizer,
            options: &IndexMergeOptions::default(),
        },
    )
    .await
    .expect("purge");

    // BETA's only source died with the document: the row, its tracking row and
    // the entity vector are gone.
    assert_eq!(report.deleted_entities, vec!["BETA".to_string()]);
    assert!(
        fixture
            .graph
            .get_node("BETA")
            .await
            .expect("read")
            .is_none(),
        "the dangling node is removed"
    );
    let beta_vector = compute_mdhash_id("BETA", "ent-");
    assert!(
        fixture
            .entities_vdb
            .query("labs", 10)
            .await
            .expect("query")
            .iter()
            .all(|hit| hit.id != beta_vector),
        "the entity vector row is removed"
    );
    assert!(
        fixture
            .tracking
            .entity_chunks
            .get_by_id("BETA")
            .await
            .expect("read")
            .is_none(),
        "its tracking row goes too"
    );

    // ACME still has chunk-b: rebuilt, not deleted.
    assert_eq!(report.rebuilt_entities, vec!["ACME".to_string()]);
    let acme = fixture
        .graph
        .get_node("ACME")
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(
        acme["source_id"].as_str().expect("source"),
        "chunk-b<SEP>chunk-a",
        "the reference merges the new source id before the stored one"
    );

    // The relation had one source only, so it is deleted with both vector ids.
    assert_eq!(report.deleted_relations, vec!["ACME<SEP>BETA".to_string()]);
    assert!(
        fixture
            .graph
            .get_edge("ACME", "BETA")
            .await
            .expect("read")
            .is_none(),
        "the edge is gone"
    );
    for id in [
        compute_mdhash_id("ACMEBETA", "rel-"),
        compute_mdhash_id("BETAACME", "rel-"),
    ] {
        assert!(
            fixture
                .relationships_vdb
                .query("owns", 10)
                .await
                .expect("query")
                .iter()
                .all(|hit| hit.id != id),
            "both relation vector directions are removed"
        );
    }

    // Chunk rows and their cache references are gone.
    assert_eq!(report.deleted_chunks, 1);
    assert!(fixture
        .text_chunks
        .get_by_id("chunk-a")
        .await
        .expect("read")
        .is_none());
    assert!(
        fixture
            .llm_cache
            .get_by_id("default:extract:hash-1")
            .await
            .expect("read")
            .is_none(),
        "the cache reference dies with the chunk"
    );
    assert!(
        fixture
            .full_docs
            .get_by_id("doc-1")
            .await
            .expect("read")
            .is_none(),
        "the document row is gone"
    );
}

#[tokio::test]
async fn deleting_an_unknown_document_is_a_no_op() {
    let fixture = fixture();
    let report = delete_document(
        "doc-missing",
        &PurgeStores {
            graph: &fixture.graph,
            entities_vdb: &fixture.entities_vdb,
            relationships_vdb: None,
            text_chunks: &fixture.text_chunks,
            full_docs: &fixture.full_docs,
            full_entities: &fixture.full_entities,
            full_relations: &fixture.full_relations,
            llm_cache: &fixture.llm_cache,
            tracking: &fixture.tracking,
            llm: &fixture.llm,
            tokenizer: &fixture.tokenizer,
            options: &IndexMergeOptions::default(),
        },
    )
    .await
    .expect("purge");

    assert!(report.deleted_entities.is_empty());
    assert!(report.deleted_relations.is_empty());
    assert_eq!(report.deleted_chunks, 0);
}

#[tokio::test]
async fn document_without_chunks_drops_only_its_own_rows() {
    let fixture = fixture();
    fixture
        .full_docs
        .upsert(vec![("doc-empty".to_string(), json!({"content": "x"}))])
        .await
        .expect("docs");
    fixture
        .full_entities
        .upsert(vec![(
            "doc-empty".to_string(),
            json!({"entities": ["NEVER_SEEN"]}),
        )])
        .await
        .expect("anchor");

    let report = delete_document(
        "doc-empty",
        &PurgeStores {
            graph: &fixture.graph,
            entities_vdb: &fixture.entities_vdb,
            relationships_vdb: None,
            text_chunks: &fixture.text_chunks,
            full_docs: &fixture.full_docs,
            full_entities: &fixture.full_entities,
            full_relations: &fixture.full_relations,
            llm_cache: &fixture.llm_cache,
            tracking: &fixture.tracking,
            llm: &fixture.llm,
            tokenizer: &fixture.tokenizer,
            options: &IndexMergeOptions::default(),
        },
    )
    .await
    .expect("purge");

    assert_eq!(report.deleted_chunks, 0);
    assert!(report.deleted_entities.is_empty());
    assert!(fixture
        .full_docs
        .get_by_id("doc-empty")
        .await
        .expect("read")
        .is_none());
    assert!(
        fixture
            .full_entities
            .get_by_id("doc-empty")
            .await
            .expect("read")
            .is_none(),
        "the anchor row goes with the document"
    );
}
