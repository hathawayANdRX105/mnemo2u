//! Offline full-chain smoke over the **durable** backends (T12): ingest →
//! communities → reports → three query modes, then a second "process" over the
//! same files, then losing the derived vector index and rebuilding it from the
//! truth layer.
//!
//! Everything runs against real backends (turso KV + turso graph + lancedb) with
//! a routed mock LLM: no network, no real model download.

use std::sync::Arc;

use mnemo2u::core::rag::{QueryMode, QueryParam};
use mnemo2u::core::traits::Embedder;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockEmbedder;
use mnemo2u::llm::mock::RoutedLlm;
use mnemo2u::pipeline::{InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::lancedb::LanceVector;
use mnemo2u::store::memory::MemoryKv;
use mnemo2u::store::repair::RepairQueue;
use mnemo2u::store::turso::TursoKv;
use mnemo2u::store::turso_graph::TursoGraph;
use mnemo2u::Tokenizer;

const DOCS: [&str; 2] = [
    "ACME Corporation builds warehouse robots. ACME is an organization that ships cobots.",
    "BETA LABS makes delivery drones. BETA LABS competes with ACME Corporation.",
];

const EXTRACTION: &str = "(\"entity\"<|>\"ACME\"<|>\"ORGANIZATION\"<|>\"ACME builds cobots.\")##(\"entity\"<|>\"BETA LABS\"<|>\"ORGANIZATION\"<|>\"BETA builds drones.\")##(\"relationship\"<|>\"ACME\"<|>\"BETA LABS\"<|>\"competes with\"<|>\"they compete\"<|>2.0)<|COMPLETE|>";
const GLEAN: &str =
    "(\"entity\"<|>\"ACME\"<|>\"ORGANIZATION\"<|>\"Acme ships cobots to warehouses.\")<|COMPLETE|>";
const REPORT: &str = r#"{"title": "t", "summary": "s", "rating": 7.5, "rating_explanation": "e", "findings": [{"summary": "f", "explanation": "e"}]}"#;
const ANSWER: &str = "answer";

fn routed_llm() -> CachedLlm {
    let routed = Arc::new(RoutedLlm::new(
        "mock-best",
        vec![
            ("identify all entities".to_string(), EXTRACTION.to_string()),
            ("MANY entities were missed".to_string(), GLEAN.to_string()),
            (
                "general information discovery".to_string(),
                REPORT.to_string(),
            ),
            ("\"points\": [".to_string(), "[\"x\"]".to_string()),
            ("Multiple Paragraphs".to_string(), ANSWER.to_string()),
        ],
    ));
    CachedLlm::new(routed, Arc::new(MemoryKv::new()))
}

fn query_param(mode: QueryMode) -> QueryParam {
    QueryParam {
        mode,
        only_need_context: false,
        ..QueryParam::default()
    }
}

/// One "process": its own handles over the same files.
async fn build(truth: &str, vectors: &str) -> Pipeline {
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new());
    Pipeline {
        full_docs: Arc::new(TursoKv::open(truth, "full_docs").await.expect("full_docs")),
        text_chunks: Arc::new(
            TursoKv::open(truth, "text_chunks")
                .await
                .expect("text_chunks"),
        ),
        community_reports: Arc::new(
            TursoKv::open(truth, "community_reports")
                .await
                .expect("community_reports"),
        ),
        graph: Arc::new(
            TursoGraph::open(truth, "entity_graph")
                .await
                .expect("graph"),
        ),
        entities_vdb: Arc::new(
            LanceVector::open(vectors, "entities_vdb", embedder.clone(), 0.2)
                .await
                .expect("entities_vdb"),
        ),
        chunks_vdb: Some(Arc::new(
            LanceVector::open(vectors, "text_chunks", embedder, 0.2)
                .await
                .expect("chunks_vdb"),
        )),
        llm: routed_llm(),
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        options: PipelineOptions {
            enable_naive_rag: true,
            ..PipelineOptions::default()
        },
        repair: Arc::new(RepairQueue::new(Arc::new(
            TursoKv::open(truth, "repair_queue")
                .await
                .expect("repair queue"),
        ))),
    }
}

#[tokio::test]
async fn durable_ingest_restart_and_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let truth = dir
        .path()
        .join("truth.db")
        .to_str()
        .expect("utf8")
        .to_string();
    let vectors = dir
        .path()
        .join("vectors")
        .to_str()
        .expect("utf8")
        .to_string();

    // ---- process 1: ingest ------------------------------------------------
    let first = build(&truth, &vectors).await;
    for doc in DOCS {
        let outcome = first.insert(vec![doc.to_string()]).await.expect("insert");
        assert!(
            matches!(outcome, InsertOutcome::Inserted { .. }),
            "durable insert must complete: {outcome:?}"
        );
    }
    first.flush().await.expect("flush");
    assert!(
        first.repair.pending().await.expect("pending").is_empty(),
        "a healthy ingest leaves nothing queued"
    );
    assert!(first.text_chunks.all_keys().await.expect("chunks").len() >= 2);
    assert!(!first
        .community_reports
        .all_keys()
        .await
        .expect("reports")
        .is_empty());

    // ---- process 2: fresh handles over the same files ---------------------
    let second = build(&truth, &vectors).await;

    let naive = second
        .query("ACME cobots", &query_param(QueryMode::Naive))
        .await
        .expect("naive query");
    assert!(
        naive.contains("ACME"),
        "naive mode from persisted chunks: {naive}"
    );

    let local = second
        .query("ACME", &query_param(QueryMode::Local))
        .await
        .expect("local query");
    assert!(
        local.contains("ACME"),
        "local mode from the persisted graph: {local}"
    );

    let global = second
        .query("competition", &query_param(QueryMode::Global))
        .await
        .expect("global query");
    assert!(
        global.contains("f") || global.contains("s"),
        "global mode from the persisted reports: {global}"
    );

    // ---- derived index loss → rebuild from truth --------------------------
    std::fs::remove_dir_all(&vectors).expect("drop the derived vector index");
    let third = build(&truth, &vectors).await;
    let rebuilt = third.rebuild_chunk_vectors().await.expect("rebuild");
    assert!(
        rebuilt >= 2,
        "rebuild rewrites every truth chunk: {rebuilt}"
    );

    let restored = third
        .query("ACME cobots", &query_param(QueryMode::Naive))
        .await
        .expect("naive query after rebuild");
    assert_eq!(
        naive, restored,
        "rebuild from truth restores the same retrieval"
    );
}
