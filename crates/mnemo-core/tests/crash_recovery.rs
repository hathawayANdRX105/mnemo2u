//! Crash-recovery — crash-point injection harness (doc02 §8-3: shared by TAC7,
//! T7, and the T10 leftovers; doc02 §3 T10/T1/T7 criteria).
//!
//! Journal decision (doc02 §8-3): **no journal table is introduced** — crash
//! resume rides on the *idempotent delete path* (幂等续删):
//! - an interrupted insert leaves the Phase-0 anchors plus every derived row,
//!   because the truth commit (`full_docs`/`text_chunks`, `pipeline.rs:370,392`)
//!   is the commit point and the retry re-drives from the anchors;
//! - an interrupted `delete_document` keeps the Phase-0 anchors until the
//!   final step (`purge.rs:179-182`), so the re-run recomputes the same
//!   candidate set, and every remove op tolerates absent keys
//!   (`MemoryKv::remove` / `MemoryVector::remove` / `MemoryGraph::remove_node`
//!   all ignore missing ids, `store/memory.rs`);
//! - vector upserts are id-upserts (`MemoryVector::upsert` replaces the row of
//!   the same id), so a replay can never duplicate a row.
//!
//! Fault injection stays entirely in this file (the `FailingOnceVector` idiom
//! from `tests/repair_queue.rs`, generalized to a call index): the wrappers
//! hold the concrete store plus an atomic counter and fail exactly the first
//! N calls of one method, forwarding everything else. No `MutexGuard` is ever
//! held across `.await` (counters are atomics only), so the `async_trait`
//! futures stay `Send`.
//!
//! Chosen injection points (write order read from the source):
//! 1. `turn_commit_survives_derived_write_failure`
//!    — `entities_vdb.upsert` call #1: the `upsert_entity_vectors` batch in
//!    `pipeline.rs:332-341` → `merge.rs:712-733` queues the failed rows in
//!    the repair queue and the ingest still commits. The session commit
//!    (`commit_turns`, `ingest.rs:115-187`; write order docs → chunks →
//!    watermark at `ingest.rs:178-185`) then advances the watermark, and
//!    `flush` replays the queued derived rows.
//! 2. `turn_commit_rolls_back_before_truth_write`
//!    — `text_chunks.upsert` call #1 inside `commit_turns` (`ingest.rs:179`).
//!    The truth writes are one ordered sequence, so a failure between them
//!    leaves the watermark behind the data, never ahead (`ingest.rs:175-177`),
//!    and the retry re-drives the same range.
//! 3. `interrupted_insert_converges_on_rerun`
//!    — `full_docs.upsert` call #1: the first truth commit write
//!    (`pipeline.rs:370`). Everything derived is already stored (anchors
//!    `pipeline.rs:249-267`, graph `pipeline.rs:272-317`, tracking
//!    `pipeline.rs:320-328`, vectors `pipeline.rs:332-351`); the error
//!    propagates and the re-run re-merges idempotently, extraction cached.
//! 4. `interrupted_delete_converges_on_rerun`
//!    — `entities_vdb.remove` call #1: the exclusive entity's vector row
//!    (`purge.rs:131-134`), the first vector remove of the cascade. The
//!    re-run continues the delete; no journal, idempotent continuation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use mnemo2u::core::text::compute_mdhash_id;
use mnemo2u::core::traits::{
    GraphStore, KvStore, Result as StoreResult, StoreError, VectorHit, VectorRow, VectorStore,
};
use mnemo2u::index::ingest::{SessionIngest, SessionTurn, Turn};
use mnemo2u::index::merge::IndexMergeOptions;
use mnemo2u::index::purge::{delete_document, PurgeStores};
use mnemo2u::index::tracking::TrackingStores;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::{MockEmbedder, RoutedLlm};
use mnemo2u::pipeline::{InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::memory::{MemoryGraph, MemoryKv, MemoryVector};
use mnemo2u::store::repair::{RepairQueue, RepairTarget};
use mnemo2u::Tokenizer;

// --------------------------------------------------------------------------
// Corpus + routing (RoutedLlm matches a phrase contained in prompt+system)
// --------------------------------------------------------------------------

/// Gleaning runs once per chunk (`extract.rs:380-408`); route it to a
/// response that adds nothing.
const GLEANING_EMPTY: &str = "<|COMPLETE|>";

/// Test 3 corpus: two documents, four entities and two relations, each
/// element owned by exactly one document (so re-merging the same records is
/// a strict-dedup no-op — see the re-merge contract note above test 3).
const DOC_A: &str = "ALPHA LABS builds robots in Geneva. ALPHA LABS is a company.";
const DOC_B: &str = "ACME ships software in Zurich. ACME is a company.";
const EXTRACTION_A: &str = "entity<|#|>ALPHA LABS<|#|>organization<|#|>Alpha Labs builds robots.\n\
entity<|#|>GENEVA<|#|>location<|#|>Geneva hosts Alpha Labs.\n\
relation<|#|>ALPHA LABS<|#|>GENEVA<|#|>based_in<|#|>ALPHA LABS is based in GENEVA.\n\
<|COMPLETE|>";
const EXTRACTION_B: &str = "entity<|#|>ACME<|#|>organization<|#|>ACME ships software.\n\
entity<|#|>ZURICH<|#|>location<|#|>Zurich hosts ACME.\n\
relation<|#|>ACME<|#|>ZURICH<|#|>located_in<|#|>ACME is located in ZURICH.\n\
<|COMPLETE|>";

/// Test 1 corpus: two session turns, one single-entity extraction each.
const TURN_A: &str = "ACME builds cobots. ACME is an organization.";
const TURN_B: &str = "NADIA ships drones. NADIA is an engineer.";
const TURN_EXTRACTION_A: &str =
    "entity<|#|>ACME<|#|>organization<|#|>ACME builds cobots.\n<|COMPLETE|>";
const TURN_EXTRACTION_B: &str =
    "entity<|#|>NADIA<|#|>engineer<|#|>Nadia ships drones.\n<|COMPLETE|>";

/// Test 4 corpus: NADIA is owned by both documents (rebuild branch), every
/// other element is owned by exactly one (delete branch).
const DOC1: &str = "ALPHA LABS builds robots. NADIA leads the ALPHA team.";
const DOC2: &str = "ACME ships software. NADIA consults for ACME.";
const EXTRACTION_1: &str = "entity<|#|>ALPHA LABS<|#|>organization<|#|>Alpha Labs builds robots.\n\
entity<|#|>NADIA<|#|>person<|#|>Nadia leads the Alpha team.\n\
relation<|#|>ALPHA LABS<|#|>NADIA<|#|>employ<|#|>ALPHA LABS hires NADIA.\n\
<|COMPLETE|>";
const EXTRACTION_2: &str = "entity<|#|>ACME<|#|>organization<|#|>ACME ships software.\n\
entity<|#|>NADIA<|#|>person<|#|>Nadia consults for ACME.\n\
relation<|#|>ACME<|#|>NADIA<|#|>retain<|#|>ACME retains NADIA.\n\
<|COMPLETE|>";

/// Exact vector-row contents the delete test probes. `entity_vector_rows`
/// stores `"{name}\n{description}"` (`merge.rs:650-660`) and
/// `relation_vector_rows` stores `"{keywords}\t{src}\n{tgt}\n{description}"`
/// (`merge.rs:679-684`); each description below is the tier-1 join of that
/// document's single fragment.
const ALPHA_ENTITY_CONTENT: &str = "ALPHA LABS\nAlpha Labs builds robots.";
const NADIA_ENTITY_CONTENT: &str =
    "NADIA\nNadia leads the Alpha team.<SEP>Nadia consults for ACME.";
const ACME_ENTITY_CONTENT: &str = "ACME\nACME ships software.";
const ACME_NADIA_RELATION_CONTENT: &str = "retain\tACME\nNADIA\nACME retains NADIA.";
const ALPHA_NADIA_RELATION_CONTENT: &str = "employ\tALPHA LABS\nNADIA\nALPHA LABS hires NADIA.";

// --------------------------------------------------------------------------
// Fault-injection wrappers (test-local; no src change, no guard across await)
// --------------------------------------------------------------------------

/// `VectorStore` wrapper failing its first N `upsert` calls.
struct FaultyVectorUpsert {
    inner: Arc<MemoryVector>,
    upsert_calls: AtomicUsize,
    fail_first: usize,
}

#[async_trait]
impl VectorStore for FaultyVectorUpsert {
    async fn upsert(&self, rows: Vec<VectorRow>) -> StoreResult<()> {
        let call = self.upsert_calls.fetch_add(1, Ordering::SeqCst);
        if call < self.fail_first {
            return Err(StoreError::Backend(
                "injected derived vector failure".to_string(),
            ));
        }
        self.inner.upsert(rows).await
    }

    async fn query(&self, query: &str, top_k: usize) -> StoreResult<Vec<VectorHit>> {
        self.inner.query(query, top_k).await
    }

    async fn remove(&self, ids: &[String]) -> StoreResult<()> {
        self.inner.remove(ids).await
    }

    async fn index_done(&self) -> StoreResult<()> {
        self.inner.index_done().await
    }
}

fn faulty_vector_upsert(fail_first: usize) -> Arc<dyn VectorStore> {
    Arc::new(FaultyVectorUpsert {
        inner: Arc::new(MemoryVector::new(Arc::new(MockEmbedder::new()), 0.2)),
        upsert_calls: AtomicUsize::new(0),
        fail_first,
    })
}

/// `VectorStore` wrapper failing its first N `remove` calls — the
/// interrupted-delete injection (the exclusive element's vector row,
/// `purge.rs:131-134`).
struct FaultyVectorRemove {
    inner: Arc<MemoryVector>,
    remove_calls: AtomicUsize,
    fail_first: usize,
}

#[async_trait]
impl VectorStore for FaultyVectorRemove {
    async fn upsert(&self, rows: Vec<VectorRow>) -> StoreResult<()> {
        self.inner.upsert(rows).await
    }

    async fn query(&self, query: &str, top_k: usize) -> StoreResult<Vec<VectorHit>> {
        self.inner.query(query, top_k).await
    }

    async fn remove(&self, ids: &[String]) -> StoreResult<()> {
        let call = self.remove_calls.fetch_add(1, Ordering::SeqCst);
        if call < self.fail_first {
            return Err(StoreError::Backend(
                "injected derived vector failure".to_string(),
            ));
        }
        self.inner.remove(ids).await
    }

    async fn index_done(&self) -> StoreResult<()> {
        self.inner.index_done().await
    }
}

fn faulty_vector_remove(fail_first: usize) -> Arc<dyn VectorStore> {
    Arc::new(FaultyVectorRemove {
        inner: Arc::new(MemoryVector::new(Arc::new(MockEmbedder::new()), 0.2)),
        remove_calls: AtomicUsize::new(0),
        fail_first,
    })
}

/// `KvStore` wrapper failing its first N `upsert` calls (truth-commit
/// injection). Reads forward untouched, so `filter_keys` re-drive works.
struct FaultyKvUpsert {
    inner: Arc<MemoryKv>,
    upsert_calls: AtomicUsize,
    fail_first: usize,
}

#[async_trait]
impl KvStore for FaultyKvUpsert {
    async fn all_keys(&self) -> StoreResult<Vec<String>> {
        self.inner.all_keys().await
    }

    async fn get_by_id(&self, id: &str) -> StoreResult<Option<Value>> {
        self.inner.get_by_id(id).await
    }

    async fn get_by_ids(&self, ids: &[String]) -> StoreResult<Vec<Option<Value>>> {
        self.inner.get_by_ids(ids).await
    }

    async fn filter_keys(&self, ids: &[String]) -> StoreResult<Vec<String>> {
        self.inner.filter_keys(ids).await
    }

    async fn upsert(&self, rows: Vec<(String, Value)>) -> StoreResult<()> {
        let call = self.upsert_calls.fetch_add(1, Ordering::SeqCst);
        if call < self.fail_first {
            return Err(StoreError::Backend(
                "injected truth commit failure".to_string(),
            ));
        }
        self.inner.upsert(rows).await
    }

    async fn drop_all(&self) -> StoreResult<()> {
        self.inner.drop_all().await
    }

    async fn remove(&self, keys: &[String]) -> StoreResult<()> {
        self.inner.remove(keys).await
    }

    async fn index_done(&self) -> StoreResult<()> {
        self.inner.index_done().await
    }
}

fn faulty_kv_upsert(inner: Arc<MemoryKv>, fail_first: usize) -> Arc<dyn KvStore> {
    Arc::new(FaultyKvUpsert {
        inner,
        upsert_calls: AtomicUsize::new(0),
        fail_first,
    })
}

// --------------------------------------------------------------------------
// World assembly (same shape as tests/pipeline_e2e.rs; slots are injectable)
// --------------------------------------------------------------------------

struct World {
    pipeline: Pipeline,
    routed: Arc<RoutedLlm>,
    llm_cache: Arc<MemoryKv>,
    repair: Arc<RepairQueue>,
    /// The store the pipeline writes through (possibly a faulty wrapper).
    full_docs: Arc<dyn KvStore>,
    text_chunks: Arc<MemoryKv>,
    graph: Arc<MemoryGraph>,
    entities_vdb: Arc<dyn VectorStore>,
    relationships_vdb: Option<Arc<dyn VectorStore>>,
    tracking: TrackingStores,
    full_entities: Arc<MemoryKv>,
    full_relations: Arc<MemoryKv>,
}

fn plain_vector() -> Arc<dyn VectorStore> {
    Arc::new(MemoryVector::new(Arc::new(MockEmbedder::new()), 0.2))
}

fn build_world(
    rules: Vec<(String, String)>,
    entities_vdb: Option<Arc<dyn VectorStore>>,
    relationships_vdb: Option<Arc<dyn VectorStore>>,
    full_docs_fault: Option<(Arc<MemoryKv>, Arc<dyn KvStore>)>,
) -> World {
    let routed = Arc::new(RoutedLlm::new("mock-best", rules));
    let llm_cache = Arc::new(MemoryKv::new());
    let llm = CachedLlm::new(routed.clone(), llm_cache.clone());
    let repair = Arc::new(RepairQueue::new(Arc::new(MemoryKv::new())));

    let full_docs_plain = Arc::new(MemoryKv::new());
    let full_docs: Arc<dyn KvStore> = match full_docs_fault {
        Some((_, faulty)) => faulty,
        None => full_docs_plain,
    };
    let text_chunks = Arc::new(MemoryKv::new());
    let graph = Arc::new(MemoryGraph::new());
    let entities_vdb = entities_vdb.unwrap_or_else(plain_vector);
    let tracking = TrackingStores {
        entity_chunks: Arc::new(MemoryKv::new()),
        relation_chunks: Arc::new(MemoryKv::new()),
    };
    let full_entities = Arc::new(MemoryKv::new());
    let full_relations = Arc::new(MemoryKv::new());

    let pipeline = Pipeline {
        full_docs: full_docs.clone(),
        text_chunks: text_chunks.clone(),
        community_reports: Arc::new(MemoryKv::new()),
        graph: graph.clone(),
        entities_vdb: entities_vdb.clone(),
        relationships_vdb: relationships_vdb.clone(),
        tracking: tracking.clone(),
        full_entities: full_entities.clone(),
        full_relations: full_relations.clone(),
        chunks_vdb: None,
        llm,
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        options: PipelineOptions::default(),
        repair: repair.clone(),
    };

    World {
        pipeline,
        routed,
        llm_cache,
        repair,
        full_docs,
        text_chunks,
        graph,
        entities_vdb,
        relationships_vdb,
        tracking,
        full_entities,
        full_relations,
    }
}

fn purge_stores<'a>(world: &'a World, options: &'a IndexMergeOptions) -> PurgeStores<'a> {
    PurgeStores {
        graph: world.graph.as_ref(),
        entities_vdb: world.entities_vdb.as_ref(),
        relationships_vdb: world.relationships_vdb.as_ref().map(|store| store.as_ref()),
        text_chunks: world.text_chunks.as_ref(),
        full_docs: world.full_docs.as_ref(),
        full_entities: world.full_entities.as_ref(),
        full_relations: world.full_relations.as_ref(),
        llm_cache: world.llm_cache.as_ref(),
        tracking: &world.tracking,
        llm: &world.pipeline.llm,
        tokenizer: &world.pipeline.tokenizer,
        options,
    }
}

// --------------------------------------------------------------------------
// Introspection helpers (state invariants only — never not-throw)
// --------------------------------------------------------------------------

async fn kv_state(store: &dyn KvStore) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();
    for key in store.all_keys().await.expect("kv keys") {
        rows.push((
            key.clone(),
            store
                .get_by_id(&key)
                .await
                .expect("kv row")
                .unwrap_or(Value::Null),
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

async fn graph_state(
    graph: &dyn GraphStore,
) -> (Vec<(String, Value)>, Vec<(String, String, Value)>) {
    let snapshot = graph.snapshot().await.expect("graph snapshot");
    (snapshot.nodes.clone(), snapshot.edges.clone())
}

/// Deterministic vector-state observation: hit-id list for one probe.
async fn probe_ids(store: &dyn VectorStore, probe: &str) -> Vec<String> {
    store
        .query(probe, 100)
        .await
        .expect("probe query")
        .into_iter()
        .map(|hit| hit.id)
        .collect()
}

fn ent_id(name: &str) -> String {
    compute_mdhash_id(name, "ent-")
}

fn rel_ids(src: &str, tgt: &str) -> (String, String) {
    (
        compute_mdhash_id(&format!("{src}{tgt}"), "rel-"),
        compute_mdhash_id(&format!("{tgt}{src}"), "rel-"),
    )
}

/// The chunk row key of one committed document row, read from the truth
/// layer (`text_chunks.full_doc_id`) — chunk ids hash the tokenized chunk
/// text (`graph/chunk.rs:82`), so read the row back instead of re-deriving
/// the tokenizer round trip.
async fn chunk_key_for_doc(text_chunks: &dyn KvStore, doc_id: &str) -> String {
    for (key, row) in kv_state(text_chunks).await {
        if row.get("full_doc_id").and_then(Value::as_str) == Some(doc_id) {
            return key;
        }
    }
    panic!("no chunk row for {doc_id}");
}

fn turn(conversation: &str, id: &str, role: &str, content: &str, timestamp: i64) -> SessionTurn {
    SessionTurn {
        conversation_id: conversation.to_string(),
        turn: Turn {
            turn_id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp,
        },
    }
}

/// Turn chunk id — the same derivation `rows_for_turns` uses
/// (`ingest.rs:90-110`).
fn turn_key(conversation: &str, id: &str) -> String {
    compute_mdhash_id(&format!("{conversation}\u{1}{id}"), "turn-")
}

fn turn_rules() -> Vec<(String, String)> {
    vec![
        (
            "ACME builds cobots".to_string(),
            TURN_EXTRACTION_A.to_string(),
        ),
        (
            "NADIA ships drones".to_string(),
            TURN_EXTRACTION_B.to_string(),
        ),
        (
            "missed or incorrectly formatted".to_string(),
            GLEANING_EMPTY.to_string(),
        ),
    ]
}

fn cascade_rules() -> Vec<(String, String)> {
    vec![
        (
            "ALPHA LABS builds robots.".to_string(),
            EXTRACTION_1.to_string(),
        ),
        ("ACME ships software.".to_string(), EXTRACTION_2.to_string()),
        (
            "missed or incorrectly formatted".to_string(),
            GLEANING_EMPTY.to_string(),
        ),
    ]
}

fn exclusive_rules() -> Vec<(String, String)> {
    vec![
        (
            "ALPHA LABS builds robots in Geneva".to_string(),
            EXTRACTION_A.to_string(),
        ),
        (
            "ACME ships software in Zurich".to_string(),
            EXTRACTION_B.to_string(),
        ),
        (
            "missed or incorrectly formatted".to_string(),
            GLEANING_EMPTY.to_string(),
        ),
    ]
}

/// The watermark row value `commit_turns` stores for the last closed turn
/// (`ingest.rs:164-173,180-185`).
fn watermark_json(last_turn: &str, last_timestamp: i64) -> Value {
    json!({ "last_turn_id": last_turn, "last_timestamp": last_timestamp })
}

// --------------------------------------------------------------------------
// 1. T10 + T11: the derived vector write fails, the truth commit and the
//    watermark survive; `flush` replays the queued derived rows.
// --------------------------------------------------------------------------

/// Injection point: `entities_vdb.upsert` call #1 (`pipeline.rs:332-341`
/// batch → `merge.rs:712-733` queues one `EntityVector` item per row).
/// Assertions:
/// - the ingest still commits (outcome counts + `full_docs` / `text_chunks`
///   rows) — the derived failure is queued, not fatal;
/// - `commit_turns` returns 2 fresh turns and the watermark row advances to
///   the last turn **with the commit** (`ingest.rs:178-185`), while no
///   watermark row existed before the commit;
/// - both turn rows are present and carry the session metadata;
/// - the failed derived writes are in the repair queue: 2 pending items,
///   both `RepairTarget::EntityVector`, ids `entity_vector:<ent-…>`
///   (`repair.rs:87-99`);
/// - before `flush` the entity vector rows are absent;
/// - `flush` replay: pending → 0 and both vector rows present exactly once
///   (probe lists hold one id each; `MemoryVector::upsert` is an id-upsert,
///   so a replay cannot duplicate a row).
#[tokio::test]
async fn turn_commit_survives_derived_write_failure() {
    let world = build_world(turn_rules(), Some(faulty_vector_upsert(1)), None, None);

    // The truth rows commit last (`pipeline.rs:370,392`), so the failed
    // derived batch must not lose the ingest.
    let outcome = world
        .pipeline
        .insert(vec![TURN_A.to_string(), TURN_B.to_string()])
        .await
        .expect("derived failures are queued, they never abort the ingest");
    assert_eq!(
        outcome,
        InsertOutcome::Inserted {
            docs: 2,
            chunks: 2,
            entities: 2,
            relations: 0
        }
    );

    let watermarks = Arc::new(MemoryKv::new());
    let ingest = SessionIngest::new(
        world.full_docs.clone(),
        world.text_chunks.clone(),
        watermarks.clone(),
    );
    let turns = vec![
        turn("c1", "t1", "user", TURN_A, 1),
        turn("c1", "t2", "assistant", TURN_B, 2),
    ];

    // T10 crash-point criterion: before the commit the watermark may not
    // claim any turn.
    assert!(
        watermarks
            .get_by_id("c1")
            .await
            .expect("watermark read")
            .is_none(),
        "no watermark before the commit"
    );

    let committed = ingest
        .commit_turns("c1", &turns)
        .await
        .expect("truth commit");
    assert_eq!(committed, 2, "both turns are fresh");

    // The watermark advanced together with the rows it claims.
    assert_eq!(
        watermarks.get_by_id("c1").await.expect("watermark read"),
        Some(watermark_json("t2", 2)),
        "the watermark advanced with the commit"
    );

    for (key, content) in [
        (turn_key("c1", "t1"), TURN_A),
        (turn_key("c1", "t2"), TURN_B),
    ] {
        let row = world
            .text_chunks
            .get_by_id(&key)
            .await
            .expect("turn row read")
            .expect("turn row committed");
        assert_eq!(row["content"], json!(content));
        assert_eq!(row["full_doc_id"], json!("conv-c1"));
    }
    assert!(
        world
            .full_docs
            .get_by_id("c1")
            .await
            .expect("doc read")
            .is_some(),
        "the conversation doc row committed"
    );

    // The failed derived writes are queued in the truth layer.
    let pending = world.repair.pending().await.expect("pending items");
    assert_eq!(pending.len(), 2, "one queued item per failed vector row");
    assert!(
        pending
            .iter()
            .all(|item| item.target == RepairTarget::EntityVector),
        "every pending item is an entity-vector replay"
    );
    let pending_ids: Vec<String> = pending.iter().map(|item| item.id.clone()).collect();
    assert!(pending_ids.contains(&format!("entity_vector:{}", ent_id("ACME"))));
    assert!(pending_ids.contains(&format!("entity_vector:{}", ent_id("NADIA"))));

    // Still missing on the derived side.
    assert!(
        probe_ids(world.entities_vdb.as_ref(), "ACME\nACME builds cobots.")
            .await
            .is_empty(),
        "the failed entity vectors are not written yet"
    );

    // Replay on flush: derived rows complete, queue drains, nothing doubles.
    world.pipeline.flush().await.expect("flush replays");
    assert!(
        world
            .repair
            .pending()
            .await
            .expect("pending items")
            .is_empty(),
        "the queue drained"
    );
    // Collision-safe phrasing: the self row is the unique top hit, and each
    // replayed row appears exactly once.
    let acme_hits = probe_ids(world.entities_vdb.as_ref(), "ACME\nACME builds cobots.").await;
    assert_eq!(
        acme_hits.first(),
        Some(&ent_id("ACME")),
        "the replayed ACME row is present and top-ranked"
    );
    let nadia_hits = probe_ids(world.entities_vdb.as_ref(), "NADIA\nNadia ships drones.").await;
    assert_eq!(
        nadia_hits.first(),
        Some(&ent_id("NADIA")),
        "the replayed NADIA row is present and top-ranked"
    );
    assert_eq!(
        acme_hits
            .iter()
            .filter(|id| id.as_str() == ent_id("ACME").as_str())
            .count(),
        1,
        "the replay cannot duplicate the ACME row"
    );
    assert_eq!(
        nadia_hits
            .iter()
            .filter(|id| id.as_str() == ent_id("NADIA").as_str())
            .count(),
        1,
        "the replay cannot duplicate the NADIA row"
    );
}

// --------------------------------------------------------------------------
// 2. T10: the truth KV commit itself fails — no half-committed claim, and
//    the retry converges without doubling.
// --------------------------------------------------------------------------

/// Injection point: `text_chunks.upsert` call #1 inside `commit_turns`
/// (`ingest.rs:179`, between the doc upsert at `ingest.rs:178` and the
/// watermark upsert at `ingest.rs:180-185`). Assertions:
/// - the failure propagates;
/// - the watermark row does not exist and no turn chunk row exists — nothing
///   claims turns that are not committed;
/// - the conversation doc row does exist: the documented partial order
///   (docs first, watermark last) keeps the watermark *behind* the data,
///   never ahead (`ingest.rs:175-177`);
/// - the retry commits 2 turns and converges: watermark at t2, exactly the
///   two turn rows, exactly one doc row and one watermark row — no doubling
///   from the re-driven range.
#[tokio::test]
async fn turn_commit_rolls_back_before_truth_write() {
    let turns = vec![
        turn("c1", "t1", "user", TURN_A, 1),
        turn("c1", "t2", "assistant", TURN_B, 2),
    ];
    let chunk_rows = Arc::new(MemoryKv::new());
    let full_docs = Arc::new(MemoryKv::new());
    let watermarks = Arc::new(MemoryKv::new());

    let ingest = SessionIngest::new(
        full_docs.clone(),
        faulty_kv_upsert(chunk_rows.clone(), 1),
        watermarks.clone(),
    );

    let error = ingest
        .commit_turns("c1", &turns)
        .await
        .expect_err("the chunk-step failure propagates");
    assert!(
        error.to_string().contains("injected truth commit failure"),
        "the commit error surfaces: {error}"
    );

    // Nothing may claim the uncommitted turns.
    assert!(
        watermarks
            .get_by_id("c1")
            .await
            .expect("watermark read")
            .is_none(),
        "the watermark did not advance"
    );
    assert!(chunk_rows
        .get_by_id(&turn_key("c1", "t1"))
        .await
        .expect("turn row read")
        .is_none());
    assert!(chunk_rows
        .get_by_id(&turn_key("c1", "t2"))
        .await
        .expect("turn row read")
        .is_none());
    // Documented partial order: the doc row was written before the failed
    // chunk commit (`ingest.rs:178`).
    assert!(
        full_docs.get_by_id("c1").await.expect("doc read").is_some(),
        "the doc row is ahead of the failed chunk commit, the watermark never is"
    );

    // Retry converges (upsert #2 succeeds).
    let committed = ingest
        .commit_turns("c1", &turns)
        .await
        .expect("the retry re-drives the same range");
    assert_eq!(committed, 2, "the retry commits both turns once");

    assert_eq!(
        watermarks.get_by_id("c1").await.expect("watermark read"),
        Some(watermark_json("t2", 2))
    );
    let mut expected_turn_rows = vec![
        (
            turn_key("c1", "t1"),
            json!({
                "tokens": 0,
                "content": TURN_A,
                "chunk_order_index": 0,
                "full_doc_id": "conv-c1",
                "file_path": "conversation/c1",
            }),
        ),
        (
            turn_key("c1", "t2"),
            json!({
                "tokens": 0,
                "content": TURN_B,
                "chunk_order_index": 0,
                "full_doc_id": "conv-c1",
                "file_path": "conversation/c1",
            }),
        ),
    ];
    expected_turn_rows.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        kv_state(chunk_rows.as_ref()).await,
        expected_turn_rows,
        "exactly the two turn rows, no doubled writes"
    );
    assert_eq!(
        kv_state(full_docs.as_ref()).await,
        vec![("c1".to_string(), json!({ "content": "" }))],
        "one conversation doc row, not two"
    );
    assert_eq!(
        kv_state(watermarks.as_ref()).await,
        vec![("c1".to_string(), watermark_json("t2", 2))],
        "one watermark row at the last turn"
    );
}

// --------------------------------------------------------------------------
// 3. T1: an insert interrupted at the commit point re-runs to a
//    byte-identical state (cache = zero LLM re-invocation).
// --------------------------------------------------------------------------

/// Re-merge contract this test relies on (documented, not assumed): with
/// every element owned by exactly one document, re-feeding the same records
/// is a strict-dedup no-op — `combine_descriptions` drops the duplicate
/// fragment (`merge.rs:270-282`), `source_id` is a dedup union
/// (`merge.rs:350-355`), weight only adds unseen sources
/// (`merge.rs:492-507`) and keywords are a set union (`merge.rs:544-567`).
///
/// Injection point: `full_docs.upsert` call #1 — the first truth commit
/// write (`pipeline.rs:370`), after the anchors (`pipeline.rs:249-267`),
/// the graph merge (`pipeline.rs:272-317`), the tracking rows
/// (`pipeline.rs:320-328`) and both vector writes (`pipeline.rs:332-351`).
/// Assertions:
/// - the error propagates out of `insert` (this is a propagating `?`; the
///   derived-write failures further down the path are queued instead);
/// - the interrupted state: anchors + graph + vector rows are present, and
///   the truth layer has no doc/chunk rows (the document is "not ingested",
///   so the retry re-drives it);
/// - the re-run converges to `Inserted` with the same counts;
/// - `routed.calls()` does not move across the re-run — extraction replays
///   from the LLM cache, zero re-invocation;
/// - the final state is byte-identical to a no-fault pipeline over the same
///   corpus: graph nodes/edges, `full_docs`, `text_chunks`, both tracking
///   stores, both anchor stores, the LLM cache rows, and the vector hit-id
///   lists for the same probes.
#[tokio::test]
async fn interrupted_insert_converges_on_rerun() {
    let clean = build_world(exclusive_rules(), None, Some(plain_vector()), None);
    let clean_outcome = clean
        .pipeline
        .insert(vec![DOC_A.to_string(), DOC_B.to_string()])
        .await
        .expect("fault-free insert");
    assert_eq!(
        clean_outcome,
        InsertOutcome::Inserted {
            docs: 2,
            chunks: 2,
            entities: 4,
            relations: 2
        }
    );

    let docs_rows = Arc::new(MemoryKv::new());
    let fault = build_world(
        exclusive_rules(),
        None,
        Some(plain_vector()),
        Some((docs_rows.clone(), faulty_kv_upsert(docs_rows.clone(), 1))),
    );

    // First attempt: everything derived is stored, the truth commit fails.
    let error = fault
        .pipeline
        .insert(vec![DOC_A.to_string(), DOC_B.to_string()])
        .await
        .expect_err("the commit-point failure propagates out of insert");
    assert!(
        error.to_string().contains("injected truth commit failure"),
        "the commit error surfaces: {error}"
    );

    let doc_a_key = compute_mdhash_id(DOC_A, "doc-");
    let doc_b_key = compute_mdhash_id(DOC_B, "doc-");

    // Interrupted state: anchors and derived rows are in, truth rows are not.
    assert!(fault
        .full_entities
        .get_by_id(&doc_a_key)
        .await
        .expect("anchor read")
        .is_some());
    assert!(fault
        .full_relations
        .get_by_id(&doc_a_key)
        .await
        .expect("anchor read")
        .is_some());
    assert!(fault
        .graph
        .get_node("ALPHA LABS")
        .await
        .expect("node read")
        .is_some());
    let alpha_hits = probe_ids(
        fault.entities_vdb.as_ref(),
        "ALPHA LABS\nAlpha Labs builds robots.",
    )
    .await;
    assert_eq!(
        alpha_hits.first(),
        Some(&ent_id("ALPHA LABS")),
        "the derived entity row was written before the commit"
    );
    assert!(
        kv_state(docs_rows.as_ref()).await.is_empty(),
        "no document row after the failed commit"
    );
    assert!(
        kv_state(fault.text_chunks.as_ref()).await.is_empty(),
        "no chunk row after the failed commit"
    );

    // Re-run: extraction replays from the cache (no new LLM calls).
    let calls_before = fault.routed.calls();
    assert!(
        calls_before > 0,
        "the first attempt really called the model"
    );
    let fault_outcome = fault
        .pipeline
        .insert(vec![DOC_A.to_string(), DOC_B.to_string()])
        .await
        .expect("the re-run commits");
    assert_eq!(fault_outcome, clean_outcome);
    assert_eq!(
        fault.routed.calls(),
        calls_before,
        "the re-run re-invoked the LLM instead of replaying the cache"
    );
    fault.pipeline.flush().await.expect("flush");
    clean.pipeline.flush().await.expect("flush");

    // Convergence: byte-identical final state against the fault-free run.
    assert_eq!(
        graph_state(fault.graph.as_ref()).await,
        graph_state(clean.graph.as_ref()).await,
        "graph nodes/edges converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.full_docs.as_ref()).await,
        kv_state(clean.full_docs.as_ref()).await,
        "document rows converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.text_chunks.as_ref()).await,
        kv_state(clean.text_chunks.as_ref()).await,
        "chunk rows converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.tracking.entity_chunks.as_ref()).await,
        kv_state(clean.tracking.entity_chunks.as_ref()).await,
        "entity tracking rows converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.tracking.relation_chunks.as_ref()).await,
        kv_state(clean.tracking.relation_chunks.as_ref()).await,
        "relation tracking rows converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.full_entities.as_ref()).await,
        kv_state(clean.full_entities.as_ref()).await,
        "entity anchors converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.full_relations.as_ref()).await,
        kv_state(clean.full_relations.as_ref()).await,
        "relation anchors converged byte-for-byte"
    );
    assert_eq!(
        kv_state(fault.llm_cache.as_ref()).await,
        kv_state(clean.llm_cache.as_ref()).await,
        "llm cache rows converged byte-for-byte"
    );
    assert!(
        fault.repair.pending().await.expect("pending").is_empty(),
        "the commit-point failure never queued a replay"
    );
    // Entity probes: the self row is the unique top hit (cosine 1.0); the
    // full hit lists must converge across the two worlds.
    for (probe, expected) in [
        (
            "ALPHA LABS\nAlpha Labs builds robots.",
            ent_id("ALPHA LABS"),
        ),
        ("ACME\nACME ships software.", ent_id("ACME")),
    ] {
        let fault_hits = probe_ids(fault.entities_vdb.as_ref(), probe).await;
        let clean_hits = probe_ids(clean.entities_vdb.as_ref(), probe).await;
        assert_eq!(
            fault_hits, clean_hits,
            "entity vector state converged for {probe}"
        );
        assert_eq!(
            fault_hits.first(),
            Some(&expected),
            "the top entity vector hit is the expected single row for {probe}"
        );
    }
    let relation_probe = "based_in\tALPHA LABS\nGENEVA\nALPHA LABS is based in GENEVA.";
    assert_eq!(
        probe_ids(
            fault
                .relationships_vdb
                .as_deref()
                .expect("relationships store"),
            relation_probe
        )
        .await,
        probe_ids(
            clean
                .relationships_vdb
                .as_deref()
                .expect("relationships store"),
            relation_probe
        )
        .await,
        "relation vector state converged"
    );
    // Both directions of the re-run relation are present (their own rows
    // are the top hits); other rows of the store may share words, so this
    // is a containment check — convergence across worlds is asserted above.
    let got_relation_rows = probe_ids(
        fault
            .relationships_vdb
            .as_deref()
            .expect("relationships store"),
        relation_probe,
    )
    .await;
    let (alpha_forward, alpha_reverse) = rel_ids("ALPHA LABS", "GENEVA");
    for expected in [alpha_forward, alpha_reverse] {
        assert!(
            got_relation_rows.contains(&expected),
            "relation vector row {expected} survived the re-run"
        );
    }
    assert!(doc_a_key != doc_b_key);
}

// --------------------------------------------------------------------------
// 4. T7: an interrupted cascade delete converges on a re-run — the journal
//    substitute (doc02 §8-3: idempotent continuation, no journal table).
// --------------------------------------------------------------------------

/// Injection point: `entities_vdb.remove` call #1 — the exclusive entity's
/// vector row, the first vector remove of the cascade (`purge.rs:131-134`,
/// after `graph.remove_node` + the tracking-row remove at `purge.rs:125-130`).
/// Assertions:
/// - the error propagates out of `delete_document`;
/// - interrupted state: the exclusive graph node and its tracking row are
///   gone, its vector row survives, and the chunk rows, the document row and
///   both Phase-0 anchors are still there (anchors are deleted last,
///   `purge.rs:179-182`) — that is what makes the candidate set recomputable;
/// - the re-run completes the cascade and the report equals a fault-free
///   delete: one deleted entity, one rebuilt (shared) entity, one deleted
///   relation, one deleted chunk;
/// - final state is identical to the fault-free delete: graph, all KV
///   stores, the LLM cache and the vector hit-id lists for the same probes;
/// - the deleted elements' vector rows are gone in both directions.
#[tokio::test]
async fn interrupted_delete_converges_on_rerun() {
    let clean = build_world(cascade_rules(), None, Some(plain_vector()), None);
    let fault = build_world(
        cascade_rules(),
        Some(faulty_vector_remove(1)),
        Some(plain_vector()),
        None,
    );

    for world in [&clean, &fault] {
        let outcome = world
            .pipeline
            .insert(vec![DOC1.to_string(), DOC2.to_string()])
            .await
            .expect("insert");
        assert_eq!(
            outcome,
            InsertOutcome::Inserted {
                docs: 2,
                chunks: 2,
                entities: 3,
                relations: 2
            }
        );
    }

    let doc1_key = compute_mdhash_id(DOC1, "doc-");
    // Chunk ids are content hashes of the tokenized chunk
    // (`graph/chunk.rs:82`), so read them back from the rows the commit
    // produced instead of re-deriving the tokenizer round trip.
    let chunk1_key = chunk_key_for_doc(fault.text_chunks.as_ref(), &doc1_key).await;
    let chunk2_key =
        chunk_key_for_doc(fault.text_chunks.as_ref(), &compute_mdhash_id(DOC2, "doc-")).await;
    assert_ne!(chunk1_key, chunk2_key);
    let options = IndexMergeOptions::default();

    let clean_report = delete_document(&doc1_key, &purge_stores(&clean, &options))
        .await
        .expect("fault-free delete");

    let first_attempt = delete_document(&doc1_key, &purge_stores(&fault, &options)).await;
    let error = first_attempt.expect_err("the vector-remove failure propagates");
    assert!(
        error
            .to_string()
            .contains("injected derived vector failure"),
        "the cascade error surfaces: {error}"
    );

    // Interrupted state: half of the exclusive element is gone, the vector
    // row is not, and everything that keeps the re-run computable is intact.
    assert!(fault
        .graph
        .get_node("ALPHA LABS")
        .await
        .expect("node read")
        .is_none());
    assert!(fault
        .tracking
        .entity_chunks
        .get_by_id("ALPHA LABS")
        .await
        .expect("tracking read")
        .is_none());
    let alpha_hits = probe_ids(fault.entities_vdb.as_ref(), ALPHA_ENTITY_CONTENT).await;
    assert_eq!(
        alpha_hits.first(),
        Some(&ent_id("ALPHA LABS")),
        "the exclusive entity's vector row survived the interruption"
    );
    assert!(
        fault
            .text_chunks
            .get_by_id(&chunk1_key)
            .await
            .expect("chunk read")
            .is_some(),
        "chunk rows go last (`purge.rs:169-173`)"
    );
    assert!(fault
        .full_docs
        .get_by_id(&doc1_key)
        .await
        .expect("doc read")
        .is_some());
    assert!(fault
        .full_entities
        .get_by_id(&doc1_key)
        .await
        .expect("anchor read")
        .is_some());
    assert!(fault
        .full_relations
        .get_by_id(&doc1_key)
        .await
        .expect("anchor read")
        .is_some());

    // Re-run: the cascade continues and converges.
    let fault_report = delete_document(&doc1_key, &purge_stores(&fault, &options))
        .await
        .expect("the re-run completes the delete");

    assert_eq!(fault_report.deleted_entities, clean_report.deleted_entities);
    assert_eq!(fault_report.rebuilt_entities, clean_report.rebuilt_entities);
    assert_eq!(
        fault_report.deleted_relations,
        clean_report.deleted_relations
    );
    assert_eq!(
        fault_report.rebuilt_relations,
        clean_report.rebuilt_relations
    );
    assert_eq!(fault_report.deleted_chunks, clean_report.deleted_chunks);
    assert_eq!(
        clean_report.deleted_entities,
        vec!["ALPHA LABS".to_string()]
    );
    assert_eq!(clean_report.rebuilt_entities, vec!["NADIA".to_string()]);
    assert_eq!(fault_report.deleted_chunks, 1);

    // Convergence against the fault-free delete.
    assert_eq!(
        graph_state(fault.graph.as_ref()).await,
        graph_state(clean.graph.as_ref()).await,
        "graph converged after the resumed delete"
    );
    let kv_pairs: [(&str, &dyn KvStore, &dyn KvStore); 7] = [
        (
            "full_docs",
            fault.full_docs.as_ref(),
            clean.full_docs.as_ref(),
        ),
        (
            "text_chunks",
            fault.text_chunks.as_ref() as &dyn KvStore,
            clean.text_chunks.as_ref() as &dyn KvStore,
        ),
        (
            "entity_chunks",
            fault.tracking.entity_chunks.as_ref() as &dyn KvStore,
            clean.tracking.entity_chunks.as_ref() as &dyn KvStore,
        ),
        (
            "relation_chunks",
            fault.tracking.relation_chunks.as_ref() as &dyn KvStore,
            clean.tracking.relation_chunks.as_ref() as &dyn KvStore,
        ),
        (
            "full_entities",
            fault.full_entities.as_ref() as &dyn KvStore,
            clean.full_entities.as_ref() as &dyn KvStore,
        ),
        (
            "full_relations",
            fault.full_relations.as_ref() as &dyn KvStore,
            clean.full_relations.as_ref() as &dyn KvStore,
        ),
        (
            "llm_cache",
            fault.llm_cache.as_ref() as &dyn KvStore,
            clean.llm_cache.as_ref() as &dyn KvStore,
        ),
    ];
    for (name, fault_store, clean_store) in kv_pairs {
        assert_eq!(
            kv_state(fault_store).await,
            kv_state(clean_store).await,
            "{name} converged after the resumed delete"
        );
    }

    // The surviving document's rows are untouched; the deleted ones are gone.
    assert_eq!(
        clean_report.deleted_chunks, 1,
        "exactly the interrupted document's chunk died"
    );
    assert!(clean
        .text_chunks
        .get_by_id(&chunk2_key)
        .await
        .expect("chunk read")
        .is_some());
    assert!(clean
        .text_chunks
        .get_by_id(&chunk1_key)
        .await
        .expect("chunk read")
        .is_none());

    // Vector state converged, and the deleted element's rows are gone in both
    // directions.
    for (probe, fault_hits, clean_hits) in [
        (
            NADIA_ENTITY_CONTENT,
            probe_ids(fault.entities_vdb.as_ref(), NADIA_ENTITY_CONTENT).await,
            probe_ids(clean.entities_vdb.as_ref(), NADIA_ENTITY_CONTENT).await,
        ),
        (
            ACME_ENTITY_CONTENT,
            probe_ids(fault.entities_vdb.as_ref(), ACME_ENTITY_CONTENT).await,
            probe_ids(clean.entities_vdb.as_ref(), ACME_ENTITY_CONTENT).await,
        ),
        (
            ALPHA_ENTITY_CONTENT,
            probe_ids(fault.entities_vdb.as_ref(), ALPHA_ENTITY_CONTENT).await,
            probe_ids(clean.entities_vdb.as_ref(), ALPHA_ENTITY_CONTENT).await,
        ),
        (
            ACME_NADIA_RELATION_CONTENT,
            probe_ids(
                fault
                    .relationships_vdb
                    .as_deref()
                    .expect("relationships store"),
                ACME_NADIA_RELATION_CONTENT,
            )
            .await,
            probe_ids(
                clean
                    .relationships_vdb
                    .as_deref()
                    .expect("relationships store"),
                ACME_NADIA_RELATION_CONTENT,
            )
            .await,
        ),
        (
            ALPHA_NADIA_RELATION_CONTENT,
            probe_ids(
                fault
                    .relationships_vdb
                    .as_deref()
                    .expect("relationships store"),
                ALPHA_NADIA_RELATION_CONTENT,
            )
            .await,
            probe_ids(
                clean
                    .relationships_vdb
                    .as_deref()
                    .expect("relationships store"),
                ALPHA_NADIA_RELATION_CONTENT,
            )
            .await,
        ),
    ] {
        assert_eq!(fault_hits, clean_hits, "vector state converged for {probe}");
    }

    let (forward, reverse) = rel_ids("ALPHA LABS", "NADIA");
    let deleted_relation_hits = probe_ids(
        fault
            .relationships_vdb
            .as_deref()
            .expect("relationships store"),
        ALPHA_NADIA_RELATION_CONTENT,
    )
    .await;
    assert!(
        !deleted_relation_hits.contains(&forward) && !deleted_relation_hits.contains(&reverse),
        "both relation vector directions were removed"
    );
    // The surviving relation kept both of its vector rows (the pair is
    // stored under two ids with identical content, `merge.rs:695-708`).
    let (acme_forward, acme_reverse) = rel_ids("ACME", "NADIA");
    let surviving_relation = probe_ids(
        clean
            .relationships_vdb
            .as_deref()
            .expect("relationships store"),
        ACME_NADIA_RELATION_CONTENT,
    )
    .await;
    assert!(
        surviving_relation.contains(&acme_forward) && surviving_relation.contains(&acme_reverse),
        "the surviving relation kept its vector rows"
    );
    assert!(
        !probe_ids(fault.entities_vdb.as_ref(), ALPHA_ENTITY_CONTENT)
            .await
            .contains(&ent_id("ALPHA LABS")),
        "the exclusive entity vector row is gone after the resumed delete"
    );
    assert!(fault.repair.pending().await.expect("pending").is_empty());
}
