//! Incremental panorama — stage doc R2 TAC8: insert A, insert B (sharing
//! one entity with A), delete A, then query B's theme in all three modes.
//!
//! The chain asserts both directions: every mode's context still carries
//! B's entity, relation and chunk evidence, and none of it carries A's
//! exclusive entity or A's chunk text. The shared entity survives the
//! delete as a rebuild (the semantics `tests/index_purge.rs` pins down),
//! while A's exclusive node, edge and vector rows go away.

use std::sync::Arc;

use serde_json::Value;

use mnemo2u::compute_mdhash_id;
use mnemo2u::core::rag::{QueryMode, QueryParam};
use mnemo2u::index::purge::{delete_document, PurgeStores};
use mnemo2u::index::tracking::TrackingStores;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::{MockEmbedder, RoutedLlm};
use mnemo2u::pipeline::{InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::memory::{MemoryGraph, MemoryKv, MemoryVector};
use mnemo2u::store::repair::RepairQueue;
use mnemo2u::{Tokenizer, GRAPH_FIELD_SEP};

const SHARED: &str = "KOSHIRO";
const A_EXCLUSIVE: &str = "VESPERINE";
const B_EXCLUSIVE: &str = "ORLOVE";

const DOC_A: &str = "VESPERINE festival: During the VESPERINE festival the city of KOSHIRO \
lights its river lanterns, and the VESPERINE troupe parades through KOSHIRO at dusk.";
const DOC_B: &str = "HARBOR LEDGER: The ORLOVE harbor district of KOSHIRO is where the ORLOVE \
dredging works began. KOSHIRO customs cleared ORLOVE cranes at the ORLOVE pier.";

/// Only A's chunk text carries this sentence; B's never repeats it.
const A_FRAGMENT: &str = "lights its river lanterns";
/// Only B's chunk text carries this one; A's never repeats it.
const B_FRAGMENT: &str = "dredging works began";
/// B's relation description — the evidence the relation arm must surface.
const B_RELATION: &str = "ORLOVE is the harbor district of KOSHIRO";

const LOCAL_QUERY: &str = "Which district hosts ORLOVE?";
const GLOBAL_QUERY: &str = "What did KOSHIRO customs clear at the ORLOVE pier?";
const NAIVE_QUERY: &str = "ORLOVE pier customs KOSHIRO";

const EXTRACTION_A: &str = "entity<|#|>KOSHIRO<|#|>location<|#|>KOSHIRO is a river city.\n\
entity<|#|>VESPERINE<|#|>event<|#|>VESPERINE is a festival troupe.\n\
relation<|#|>KOSHIRO<|#|>VESPERINE<|#|>hosts<|#|>KOSHIRO hosts the VESPERINE festival.\n\
<|COMPLETE|>";
const EXTRACTION_B: &str = "entity<|#|>KOSHIRO<|#|>location<|#|>KOSHIRO is a port city.\n\
entity<|#|>ORLOVE<|#|>organization<|#|>ORLOVE builds harbor infrastructure.\n\
relation<|#|>KOSHIRO<|#|>ORLOVE<|#|>harbor<|#|>ORLOVE is the harbor district of KOSHIRO.\n\
<|COMPLETE|>";
const KEYWORDS_LOCAL: &str =
    r#"{"high_level_keywords": ["harbor"], "low_level_keywords": ["ORLOVE"]}"#;
const KEYWORDS_GLOBAL: &str = r#"{"high_level_keywords": ["harbor"], "low_level_keywords": []}"#;

/// Extraction is keyed on a phrase from the document's own chunk text; the
/// gleaning round (continue template plus system, no chunk text) matches its
/// own rule; the keyword rules are keyed on the query text, which the
/// keyword prompt embeds verbatim.
fn routing() -> Vec<(String, String)> {
    vec![
        ("VESPERINE festival".to_string(), EXTRACTION_A.to_string()),
        ("HARBOR LEDGER".to_string(), EXTRACTION_B.to_string()),
        ("missed or incorrectly formatted".to_string(), String::new()),
        (
            "Extract entities and relationships".to_string(),
            String::new(),
        ),
        (LOCAL_QUERY.to_string(), KEYWORDS_LOCAL.to_string()),
        (
            "What did KOSHIRO customs clear".to_string(),
            KEYWORDS_GLOBAL.to_string(),
        ),
        (
            "high_level_keywords".to_string(),
            KEYWORDS_GLOBAL.to_string(),
        ),
    ]
}

/// One memory-backed pipeline; the LLM cache handle is returned because the
/// cascade delete reads and prunes it ([`PurgeStores::llm_cache`]).
fn build() -> (Pipeline, Arc<RoutedLlm>, Arc<MemoryKv>) {
    let routed = Arc::new(RoutedLlm::new("mock-best", routing()));
    let llm_cache = Arc::new(MemoryKv::new());
    let llm = CachedLlm::new(routed.clone(), llm_cache.clone());
    let embedder = Arc::new(MockEmbedder::new());
    let pipeline = Pipeline {
        full_docs: Arc::new(MemoryKv::new()),
        text_chunks: Arc::new(MemoryKv::new()),
        community_reports: Arc::new(MemoryKv::new()),
        graph: Arc::new(MemoryGraph::new()),
        entities_vdb: Arc::new(MemoryVector::new(embedder.clone(), 0.2)),
        relationships_vdb: Some(Arc::new(MemoryVector::new(embedder.clone(), 0.2))
            as Arc<dyn mnemo2u::core::traits::VectorStore>),
        chunks_vdb: Some(Arc::new(MemoryVector::new(embedder, 0.2))
            as Arc<dyn mnemo2u::core::traits::VectorStore>),
        tracking: TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        full_entities: Arc::new(MemoryKv::new()),
        full_relations: Arc::new(MemoryKv::new()),
        llm,
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        repair: Arc::new(RepairQueue::new(Arc::new(MemoryKv::new()))),
        options: PipelineOptions {
            enable_naive_rag: true,
            ..PipelineOptions::default()
        },
    };
    (pipeline, routed, llm_cache)
}

/// The chunk ids one committed document owns (`Chunk::full_doc_id`).
async fn chunk_ids_for_doc(pipeline: &Pipeline, doc_key: &str) -> Vec<String> {
    let keys = pipeline.text_chunks.all_keys().await.expect("chunk keys");
    let mut out = Vec::new();
    for key in keys {
        let row = pipeline
            .text_chunks
            .get_by_id(&key)
            .await
            .expect("chunk row");
        if row.and_then(|r| {
            r.get("full_doc_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        }) == Some(doc_key.to_string())
        {
            out.push(key);
        }
    }
    out.sort();
    out
}

async fn context(pipeline: &Pipeline, query: &str, mode: QueryMode) -> String {
    pipeline
        .query(
            query,
            &QueryParam {
                mode,
                only_need_context: true,
                ..QueryParam::default()
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{mode:?} query failed: {error}"))
}

#[tokio::test]
async fn insert_a_insert_b_delete_a_then_b_theme_queries_see_only_b() {
    let (pipeline, routed, llm_cache) = build();

    // 1. Insert A (one topic, two entities, one relation) …
    let first = pipeline
        .insert(vec![DOC_A.to_string()])
        .await
        .expect("insert A");
    assert_eq!(
        first,
        InsertOutcome::Inserted {
            docs: 1,
            chunks: 1,
            entities: 2,
            relations: 1
        }
    );
    // … then B (another topic sharing KOSHIRO with A).
    let second = pipeline
        .insert(vec![DOC_B.to_string()])
        .await
        .expect("insert B");
    assert_eq!(
        second,
        InsertOutcome::Inserted {
            docs: 1,
            chunks: 1,
            entities: 2,
            relations: 1
        }
    );

    let doc_a = compute_mdhash_id(DOC_A, "doc-");
    let doc_b = compute_mdhash_id(DOC_B, "doc-");
    let chunks_a = chunk_ids_for_doc(&pipeline, &doc_a).await;
    let chunks_b = chunk_ids_for_doc(&pipeline, &doc_b).await;
    assert_eq!(
        (chunks_a.len(), chunks_b.len()),
        (1, 1),
        "one chunk per short document"
    );
    let chunk_a = chunks_a[0].clone();
    let chunk_b = chunks_b[0].clone();

    // The shared entity as stored before the delete: the merge keeps the
    // records' sources before the stored list, so B's chunk already leads.
    let shared_before = pipeline
        .graph
        .get_node(SHARED)
        .await
        .expect("graph read")
        .expect("shared node after both inserts");
    assert_eq!(
        shared_before["source_id"].as_str().expect("sources"),
        format!("{chunk_b}{GRAPH_FIELD_SEP}{chunk_a}")
    );

    // 2. Delete A. The chunk vector rows live in `chunks_vdb`, which the
    // pipeline owns, so the cascade step drops them alongside the call —
    // otherwise the naive arm would keep serving A's text.
    let calls_before_delete = routed.calls();
    let report = delete_document(
        &doc_a,
        &PurgeStores {
            graph: &*pipeline.graph,
            entities_vdb: &*pipeline.entities_vdb,
            relationships_vdb: pipeline.relationships_vdb.as_deref(),
            text_chunks: &*pipeline.text_chunks,
            full_docs: &*pipeline.full_docs,
            full_entities: &*pipeline.full_entities,
            full_relations: &*pipeline.full_relations,
            llm_cache: llm_cache.as_ref(),
            tracking: &pipeline.tracking,
            llm: &pipeline.llm,
            tokenizer: &pipeline.tokenizer,
            options: &pipeline.options.index_merge,
        },
    )
    .await
    .expect("purge");
    pipeline
        .chunks_vdb
        .as_ref()
        .expect("naive arm enabled")
        .remove(&chunks_a)
        .await
        .expect("chunk vector delete");

    // 3. The classification: A's exclusive element dies with the document,
    // the shared entity is rebuilt.
    assert_eq!(report.deleted_entities, vec![A_EXCLUSIVE.to_string()]);
    assert_eq!(report.rebuilt_entities, vec![SHARED.to_string()]);
    assert_eq!(
        report.deleted_relations,
        vec![format!("{SHARED}{GRAPH_FIELD_SEP}{A_EXCLUSIVE}")]
    );
    assert!(
        report.rebuilt_relations.is_empty(),
        "B's relation never lost a source: {report:?}"
    );
    assert_eq!(report.deleted_chunks, 1);
    assert_eq!(
        routed.calls(),
        calls_before_delete,
        "rebuilding a two-fragment description is tier 1: no summary LLM call"
    );

    // 4. A's rows are gone from every store the delete owns.
    assert!(pipeline
        .graph
        .get_node(A_EXCLUSIVE)
        .await
        .expect("graph read")
        .is_none());
    assert!(pipeline
        .graph
        .get_edge(SHARED, A_EXCLUSIVE)
        .await
        .expect("graph read")
        .is_none());
    assert!(pipeline
        .tracking
        .entity_chunks
        .get_by_id(A_EXCLUSIVE)
        .await
        .expect("tracking read")
        .is_none());
    assert!(pipeline
        .text_chunks
        .get_by_id(&chunk_a)
        .await
        .expect("chunk read")
        .is_none());
    assert!(pipeline
        .full_docs
        .get_by_id(&doc_a)
        .await
        .expect("doc read")
        .is_none());
    assert!(pipeline
        .full_entities
        .get_by_id(&doc_a)
        .await
        .expect("anchor read")
        .is_none());
    let gone_entity = compute_mdhash_id(A_EXCLUSIVE, "ent-");
    assert!(
        pipeline
            .entities_vdb
            .query(A_EXCLUSIVE, 10)
            .await
            .expect("entity query")
            .iter()
            .all(|hit| hit.id != gone_entity),
        "A's entity vector row is removed"
    );
    let relations_vdb = pipeline.relationships_vdb.as_ref().expect("hl arm");
    for id in [
        compute_mdhash_id(&format!("{SHARED}{A_EXCLUSIVE}"), "rel-"),
        compute_mdhash_id(&format!("{A_EXCLUSIVE}{SHARED}"), "rel-"),
    ] {
        assert!(
            relations_vdb
                .query("hosts", 10)
                .await
                .expect("relation query")
                .iter()
                .all(|hit| hit.id != id),
            "both directions of A's relation vector are removed"
        );
    }
    let kept_relation = compute_mdhash_id(&format!("{SHARED}{B_EXCLUSIVE}"), "rel-");
    assert!(
        relations_vdb
            .query("harbor", 10)
            .await
            .expect("relation query")
            .iter()
            .any(|hit| hit.id == kept_relation),
        "B's relation vector rows survive the delete"
    );

    // 5. The shared entity survived as a rebuild: only B's chunk remains as
    // a source and the description keeps only the fragment B contributed —
    // A's "river city" fragment dies with A's chunk, the cached-extraction
    // rebuild the reference performs in `rebuild_knowledge_from_chunks`
    // (`operate.py:1102`).
    let shared_after = pipeline
        .graph
        .get_node(SHARED)
        .await
        .expect("graph read")
        .expect("shared entity survives A's delete");
    assert_eq!(
        shared_after["source_id"].as_str().expect("sources"),
        chunk_b
    );
    assert_eq!(
        shared_after["description"],
        Value::String("KOSHIRO is a port city.".to_string()),
        "the rebuild drops the deleted chunk's description fragment"
    );
    let shared_entity = compute_mdhash_id(SHARED, "ent-");
    assert!(
        pipeline
            .entities_vdb
            .query(SHARED, 10)
            .await
            .expect("entity query")
            .iter()
            .any(|hit| hit.id == shared_entity),
        "the rebuild re-embeds the shared entity"
    );
    let kept_entity = compute_mdhash_id(B_EXCLUSIVE, "ent-");
    assert!(
        pipeline
            .entities_vdb
            .query(B_EXCLUSIVE, 10)
            .await
            .expect("entity query")
            .iter()
            .any(|hit| hit.id == kept_entity),
        "B's entity vector row is untouched"
    );

    // 6. Query B's theme in the three modes.
    let local = context(&pipeline, LOCAL_QUERY, QueryMode::Local).await;
    let global = context(&pipeline, GLOBAL_QUERY, QueryMode::Global).await;
    let naive = context(&pipeline, NAIVE_QUERY, QueryMode::Naive).await;

    // (a) Every mode still serves B's evidence: the entity arm finds B's
    // entity plus its one-hop relation, the relation arm finds B's relation
    // and both endpoints, the chunk arm finds B's chunk.
    assert!(local.contains(B_EXCLUSIVE), "local entity: {local}");
    assert!(local.contains(B_RELATION), "local relation: {local}");
    assert!(local.contains(B_FRAGMENT), "local chunk evidence: {local}");
    assert!(global.contains(B_EXCLUSIVE), "global endpoint: {global}");
    assert!(global.contains(B_RELATION), "global relation: {global}");
    assert!(
        global.contains(B_FRAGMENT),
        "global chunk evidence: {global}"
    );
    assert!(naive.contains(B_FRAGMENT), "naive chunk: {naive}");

    // (b) No mode may serve A: its exclusive entity and its chunk text are
    // gone from the graph, the tracking rows and the vector stores.
    for (mode, rendered) in [("local", &local), ("global", &global), ("naive", &naive)] {
        assert!(
            !rendered.contains(A_EXCLUSIVE),
            "{mode} context still carries A's exclusive entity: {rendered}"
        );
        assert!(
            !rendered.contains(A_FRAGMENT),
            "{mode} context still carries A's chunk text: {rendered}"
        );
    }

    assert!(
        routed.unmatched_prompts().is_empty(),
        "every prompt must match a rule"
    );
}
