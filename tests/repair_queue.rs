//! Repair queue contract (T11): a failed derived write is queued in the truth
//! layer, replayed by `flush`, and never loses the ingest.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use mnemo2u::core::traits::{Result, StoreError, VectorHit, VectorRow, VectorStore};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::{MockEmbedder, RoutedLlm};
use mnemo2u::pipeline::{InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::memory::{MemoryKv, MemoryVector};
use mnemo2u::store::repair::RepairQueue;
use mnemo2u::Tokenizer;

const DOC: &str = "ACME Corporation builds robots. ACME is an organization.";
const EXTRACTION: &str = "(\"entity\"<|>\"ACME\"<|>\"ORGANIZATION\"<|>\"ACME builds things.\")##(\"relationship\"<|>\"ACME\"<|>\"ACME\"<|>\"self\"<|>\"loop\"<|>1.0)<|COMPLETE|>";
const GLEAN: &str =
    "(\"entity\"<|>\"BETA LABS\"<|>\"ORGANIZATION\"<|>\"Beta exists.\")<|COMPLETE|>";
const REPORT: &str = r#"{"title": "t", "summary": "s", "rating": 7.5, "rating_explanation": "e", "findings": [{"summary": "f", "explanation": "e"}]}"#;
const ANSWER: &str = "answer";

/// Wraps a vector store and fails the first `upsert`, then behaves normally.
struct FailingOnceVector {
    inner: Arc<MemoryVector>,
    failed: Arc<AtomicBool>,
}

#[async_trait]
impl VectorStore for FailingOnceVector {
    async fn upsert(&self, rows: Vec<VectorRow>) -> Result<()> {
        if !self.failed.swap(true, Ordering::SeqCst) {
            return Err(StoreError::Backend("injected derived-store failure".into()));
        }
        self.inner.upsert(rows).await
    }

    async fn query(&self, query: &str, top_k: usize) -> Result<Vec<VectorHit>> {
        self.inner.query(query, top_k).await
    }

    async fn remove(&self, ids: &[String]) -> Result<()> {
        self.inner.remove(ids).await
    }

    async fn index_done(&self) -> Result<()> {
        self.inner.index_done().await
    }
}

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

fn build_pipeline(
    entities_vdb: Arc<dyn VectorStore>,
    repair_kv: Arc<MemoryKv>,
) -> (Pipeline, Arc<RepairQueue>) {
    let repair = Arc::new(RepairQueue::new(repair_kv));
    let pipeline = Pipeline {
        full_docs: Arc::new(MemoryKv::new()),
        text_chunks: Arc::new(MemoryKv::new()),
        community_reports: Arc::new(MemoryKv::new()),
        graph: Arc::new(mnemo2u::store::memory::MemoryGraph::new()),
        entities_vdb,
        relationships_vdb: None,
        tracking: mnemo2u::index::tracking::TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        full_entities: Arc::new(MemoryKv::new()),
        full_relations: Arc::new(MemoryKv::new()),
        chunks_vdb: None,
        llm: routed_llm(),
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        options: PipelineOptions::default(),
        repair: repair.clone(),
    };
    (pipeline, repair)
}

#[tokio::test]
async fn failed_derived_write_is_queued_and_replayed() {
    let inner = Arc::new(MemoryVector::new(Arc::new(MockEmbedder::new()), 0.2));
    let failing = Arc::new(FailingOnceVector {
        inner: inner.clone(),
        failed: Arc::new(AtomicBool::new(false)),
    });
    let repair_kv = Arc::new(MemoryKv::new());
    let (pipeline, repair) = build_pipeline(failing.clone(), repair_kv.clone());

    // The vector write fails; the ingest must still finish (truth commits first).
    let outcome = pipeline
        .insert(vec![DOC.to_string()])
        .await
        .expect("insert");
    assert!(
        matches!(outcome, InsertOutcome::Inserted { entities: 2, .. }),
        "insert should complete despite the derived failure: {outcome:?}"
    );

    // ...and the failed row went to the queue instead of being dropped.
    let pending = repair.pending().await.expect("pending");
    assert_eq!(pending.len(), 2, "one queued row per entity: {pending:?}");
    assert!(pending
        .iter()
        .all(|item| item.target == mnemo2u::store::repair::RepairTarget::EntityVector));
    assert!(pending
        .iter()
        .all(|item| item.id.starts_with("entity_vector:ent-")));

    // Re-running the failing write before the replay keeps failing: the item
    // stays queued rather than being lost.
    let failing_again = Arc::new(FailingOnceVector {
        inner: Arc::new(MemoryVector::new(Arc::new(MockEmbedder::new()), 0.2)),
        failed: Arc::new(AtomicBool::new(false)),
    });
    let (pipeline_again, repair_again) = build_pipeline(failing_again.clone(), repair_kv.clone());
    pipeline_again
        .flush()
        .await
        .expect("flush with still-failing store");
    let still_queued = repair_again.pending().await.expect("pending");
    assert_eq!(
        still_queued.len(),
        1,
        "drain is per item: the replay that fails again stays queued, the healthy one leaves"
    );
    assert!(still_queued.len() == 1, "exactly one row stays queued");

    // The healthy row was already replayed into the second store.
    let replayed = failing_again
        .inner
        .query("BETA LABS", 5)
        .await
        .expect("query replayed store");
    assert_eq!(replayed.len(), 1, "the non-failing replay lands");
    assert_eq!(replayed[0].meta, json!({"entity_name": "\"BETA LABS\""}));

    // With the store healthy, flush replays the queued rows and empties the queue.
    pipeline.flush().await.expect("flush replays");
    assert!(
        repair.pending().await.expect("pending").is_empty(),
        "queue drains on success"
    );

    // The replay actually landed: the entity is retrievable through the store
    // that failed during the insert.
    let hits = inner.query("ACME", 5).await.expect("query replayed store");
    assert_eq!(hits.len(), 1, "replayed entity vector is queryable");
    assert_eq!(hits[0].meta, json!({"entity_name": "\"ACME\""}));
    drop(pipeline_again);
}

#[tokio::test]
async fn queue_survives_restart_in_truth_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join("truth.db")
        .to_str()
        .expect("utf8 path")
        .to_string();

    {
        let kv = Arc::new(
            mnemo2u::store::turso::TursoKv::open(&path, mnemo2u::store::repair::REPAIR_SCOPE)
                .await
                .expect("open"),
        );
        let queue = RepairQueue::new(kv);
        queue
            .record(
                mnemo2u::store::repair::RepairTarget::GraphNode,
                "ACME",
                json!([{"entity_name": "ACME"}]),
            )
            .await
            .expect("record");
    }

    let kv = Arc::new(
        mnemo2u::store::turso::TursoKv::open(&path, mnemo2u::store::repair::REPAIR_SCOPE)
            .await
            .expect("reopen"),
    );
    let queue = RepairQueue::new(kv);
    let pending = queue.pending().await.expect("pending");
    assert_eq!(pending.len(), 1, "queue rows are durable");
    queue.done(&[pending[0].id.clone()]).await.expect("done");
    assert!(queue.pending().await.expect("pending").is_empty());
}

#[tokio::test]
async fn recording_is_idempotent() {
    let queue = RepairQueue::new(Arc::new(MemoryKv::new()));
    let payload = json!([{"entity_name": "ACME"}]);
    for _ in 0..3 {
        queue
            .record(
                mnemo2u::store::repair::RepairTarget::GraphNode,
                "ACME",
                payload.clone(),
            )
            .await
            .expect("record");
    }
    let pending = queue.pending().await.expect("pending");
    assert_eq!(
        pending.len(),
        1,
        "the same target+name replaces instead of growing"
    );
    assert_eq!(pending[0].id, "graph_node:ACME");
    queue
        .done(&["graph_node:ACME".to_string()])
        .await
        .expect("done");
    assert!(queue.pending().await.expect("pending").is_empty());
}
