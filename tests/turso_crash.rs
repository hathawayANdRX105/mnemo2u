//! Crash-point injection around the durable fact commit — the turso leg of
//! R1.TAC6 ("未做：turso 提交前/后的崩溃点注入").
//!
//! The gateway's commit order is documented at `src/index/gateway.rs:30-42` and
//! implemented at `:386-437`: derived indexes (vector, lexical, edges) first,
//! the truth row (`FactStore`) last, the watermark last of all. PR #11 also
//! wrapped `supersede` in `BEGIN`/`COMMIT` (`store/turso_facts.rs:213-255`), so
//! a crash can never leave a half-written correction either.
//!
//! Two crash points are injected, both against the **real** durable backends:
//!
//! 1. `fact_commit_then_crash_reconverges` — the truth row's `put` fails.
//!    Nothing is marked done (the watermark never advances) and no truth row
//!    landed, so the batch is simply uncommitted even though the derived
//!    indexes already hold their rows. Re-running converges on one fact, and
//!    running again is byte-for-byte a no-op.
//! 2. `crash_between_fact_and_watermark_converges` — the fact lands and the
//!    watermark write fails. That is the dangerous direction to get wrong: the
//!    ledger must lag the data, never lead it, or the next run would skip a
//!    range that was never written. Re-running finds the row already present,
//!    skips it, and only then catches the ledger up.
//!
//! Fault injection is test-local, following the `Faulty*` idiom in
//! `tests/crash_recovery.rs:124-265`: a wrapper holds the concrete store plus
//! an atomic counter and fails exactly the first N calls of one method while
//! forwarding every read untouched, so the retry path sees the real data. No
//! `MutexGuard` is ever held across an `.await`, so the `async_trait` futures
//! stay `Send`. Assertions are **byte-level snapshots** of the fact rows, so a
//! replay that rewrote a row (bumping `revision`, or duplicating it under a
//! scope component that failed to normalise) fails loudly.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use mnemo2u::core::text::Tokenizer;
use mnemo2u::core::traits::{
    Embedder, FactStore, FactVectorStore, KvStore, Result as StoreResult, StoreError,
};
use mnemo2u::index::cost::CostOptions;
use mnemo2u::index::gateway::{FactWriteGateway, GatewayOptions};
use mnemo2u::index::ingest::{SessionTurn, Turn};
use mnemo2u::llm::mock::MockEmbedder;
use mnemo2u::store::lance_facts::LanceFactVector;
use mnemo2u::store::memory::MemoryKv;
use mnemo2u::store::turso_edges::TursoFactEdges;
use mnemo2u::store::turso_facts::TursoFactStore;
use mnemo2u::store::turso_lexical::TursoLexical;
use mnemo2u::{FactId, ScopeKey};

/// The mock embedder's dimension.
const DIM: usize = 64;

/// Long enough to clear the value gate and to embed to a distinct vector.
const TURN_A: &str = "the deploy key rotates every ninety days and the rotated value is \
stored in the vault secret manager";
const TURN_B: &str = "the nightly export writes every partition to the cold bucket before the \
retention window closes";

fn scope() -> ScopeKey {
    ScopeKey {
        workspace_id: "ws-crash".to_string(),
        session_id: None,
        task_id: None,
    }
}

/// Options with the value gates open, so the fixtures exercise the commit path
/// rather than the screening thresholds (`tests/write_gateway.rs::gate_open`).
fn gate_open() -> GatewayOptions {
    GatewayOptions {
        scope: scope(),
        cost: CostOptions {
            min_chunk_tokens: 1,
            duplicate_similarity: 1.01,
            ..CostOptions::default()
        },
        ..GatewayOptions::default()
    }
}

fn turn(turn_id: &str, content: &str, timestamp: i64) -> SessionTurn {
    SessionTurn {
        conversation_id: "conv-1".to_string(),
        turn: Turn {
            turn_id: turn_id.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            timestamp,
        },
    }
}

// --------------------------------------------------------------------------
// Fault-injection wrappers
// --------------------------------------------------------------------------

/// A one-shot fault window over one method: every call arriving after
/// [`FaultWindow::arm`] and before the window closes fails, everything else
/// forwards. Re-arming moves the window forward instead of restarting it, so
/// "crash on the Nth write" is expressible without a second wrapper — the
/// calls already through stay through.
#[derive(Default)]
struct FaultWindow {
    calls: AtomicUsize,
    fail_until: AtomicUsize,
}

impl FaultWindow {
    fn arm(&self, count: usize) {
        self.fail_until
            .store(self.calls.load(Ordering::SeqCst) + count, Ordering::SeqCst);
    }

    /// Count this call and report whether it falls inside the armed window.
    fn failing(&self) -> bool {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        call < self.fail_until.load(Ordering::SeqCst)
    }
}

/// `FactStore` wrapper failing its armed `put` calls — the crash point
/// immediately **before** the truth commit.
struct FaultyFactPut {
    inner: Arc<TursoFactStore>,
    put_window: FaultWindow,
}

impl FaultyFactPut {
    /// Fail the next `count` truth commits, leaving the ones already through
    /// alone — needed when a test wants a crash on a *later* write.
    fn arm_puts(&self, count: usize) {
        self.put_window.arm(count);
    }
}

#[async_trait]
impl FactStore for FaultyFactPut {
    async fn put(&self, fact: &mnemo2u::core::types::Fact) -> StoreResult<()> {
        if self.put_window.failing() {
            return Err(StoreError::Backend(
                "injected fact commit failure".to_string(),
            ));
        }
        self.inner.put(fact).await
    }

    async fn put_many(&self, facts: &[mnemo2u::core::types::Fact]) -> StoreResult<()> {
        // The armed window counts calls, and a batch is a run of calls: the
        // same window that fails the third `put` must fail the third fact
        // of a batch too. Otherwise the crash-recovery test would only
        // cover the single-row path.
        for fact in facts {
            if self.put_window.failing() {
                return Err(StoreError::Backend(
                    "injected fact commit failure".to_string(),
                ));
            }
        }
        self.inner.put_many(facts).await
    }

    async fn get(
        &self,
        key: &ScopeKey,
        id: &FactId,
    ) -> StoreResult<Option<mnemo2u::core::types::Fact>> {
        self.inner.get(key, id).await
    }

    async fn supersede(&self, old: &FactId, new: &mnemo2u::core::types::Fact) -> StoreResult<()> {
        self.inner.supersede(old, new).await
    }

    async fn search(
        &self,
        key: &ScopeKey,
        query: &str,
        limit: usize,
    ) -> StoreResult<Vec<(FactId, f64)>> {
        self.inner.search(key, query, limit).await
    }

    async fn scan(&self, key: &ScopeKey) -> StoreResult<Vec<mnemo2u::core::types::Fact>> {
        self.inner.scan(key).await
    }

    async fn append_audit(&self, op: &str, payload_hash: &str) -> StoreResult<()> {
        self.inner.append_audit(op, payload_hash).await
    }
}

/// `KvStore` wrapper failing its armed `upsert` calls — the crash point
/// immediately **after** the truth commit, where the watermark is written
/// (`gateway.rs:503-510`).
struct FaultyWatermarkUpsert {
    inner: Arc<MemoryKv>,
    upsert_window: FaultWindow,
}

#[async_trait]
impl KvStore for FaultyWatermarkUpsert {
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
        if self.upsert_window.failing() {
            return Err(StoreError::Backend(
                "injected watermark write failure".to_string(),
            ));
        }
        self.inner.upsert(rows).await
    }

    async fn remove(&self, keys: &[String]) -> StoreResult<()> {
        self.inner.remove(keys).await
    }

    async fn drop_all(&self) -> StoreResult<()> {
        self.inner.drop_all().await
    }

    async fn index_done(&self) -> StoreResult<()> {
        self.inner.index_done().await
    }
}

// --------------------------------------------------------------------------
// World assembly
// --------------------------------------------------------------------------

struct Harness {
    _dir: tempfile::TempDir,
    gateway: FactWriteGateway,
    /// The real durable store, read directly for the byte-level snapshots.
    facts: Arc<TursoFactStore>,
    /// The wrapper the gateway writes through.
    watermarks: Arc<dyn KvStore>,
    vector: Arc<LanceFactVector>,
    /// The wrapper the gateway writes facts through, kept so a test can arm a
    /// crash on a *later* write without disturbing the ones already through.
    fact_writer: Arc<FaultyFactPut>,
}

impl Harness {
    /// Fail the next `count` truth commits.
    fn crash_next_puts(&self, count: usize) {
        self.fact_writer.arm_puts(count);
    }
}

/// A gateway over the durable backends, with `fail_puts` failing
/// `FactStore::put` calls and `fail_watermarks` failing `KvStore::upsert`
/// calls. No judge: the zero-LLM capture path still walks the same commit
/// loop, so the injected crash lands on the real write sequence.
async fn harness(fail_puts: usize, fail_watermarks: usize) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_string_lossy().to_string();
    let facts = Arc::new(
        TursoFactStore::open(&format!("{path}/facts.db"))
            .await
            .expect("facts"),
    );
    let vector = Arc::new(
        LanceFactVector::new(&format!("{path}/vectors"), DIM)
            .await
            .expect("vector"),
    );
    let lexical = Arc::new(
        TursoLexical::new(&format!("{path}/lexical.db"))
            .await
            .expect("lexical"),
    );
    let edges = Arc::new(
        TursoFactEdges::new(&format!("{path}/edges.db"))
            .await
            .expect("edges"),
    );
    let inner_watermarks = Arc::new(MemoryKv::new());
    let watermark_window = FaultWindow::default();
    watermark_window.arm(fail_watermarks);
    let watermarks: Arc<dyn KvStore> = Arc::new(FaultyWatermarkUpsert {
        inner: inner_watermarks,
        upsert_window: watermark_window,
    });

    let put_window = FaultWindow::default();
    put_window.arm(fail_puts);

    let fact_writer = Arc::new(FaultyFactPut {
        inner: facts.clone(),
        put_window,
    });
    let gateway = FactWriteGateway::new(
        gate_open(),
        mnemo2u::index::gateway::GatewayParts {
            facts: fact_writer.clone(),
            vector: Some(vector.clone()),
            lexical: Some(lexical.clone()),
            edges: Some(edges.clone()),
            embedder: Arc::new(MockEmbedder::new()),
            judge: None,
            watermarks: watermarks.clone(),
            tombstones: Arc::new(MemoryKv::new()),
            tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        },
    );
    Harness {
        _dir: dir,
        gateway,
        facts,
        watermarks,
        vector,
        fact_writer,
    }
}

/// Byte-level snapshot of every live fact row under `scope`, sorted by its
/// serialized bytes. `search` already applies the scope predicate inside the
/// store and drops superseded rows, so this is exactly "what the truth layer
/// holds and would return".
async fn fact_bytes(facts: &dyn FactStore, key: &ScopeKey) -> String {
    let hits = facts.search(key, "", 1_000).await.expect("search");
    let mut rows = Vec::with_capacity(hits.len());
    for (id, _) in hits {
        let fact = facts
            .get(key, &id)
            .await
            .expect("get")
            .expect("a searched row reads back");
        rows.push(serde_json::to_string(&fact).expect("fact serializes"));
    }
    rows.sort();
    rows.join("\n")
}

/// The persisted watermark's `(last_turn_id, last_timestamp)`, read back from
/// the store rather than from the returned outcome. `None` when the ledger has
/// no row at all, which is the state every crash must leave the store in when
/// nothing was committed.
async fn watermark(harness: &Harness, conversation_id: &str) -> Option<(String, i64)> {
    let row = harness
        .watermarks
        .get_by_id(conversation_id)
        .await
        .expect("watermark read")?;
    Some((
        row["last_turn_id"].as_str().expect("turn id").to_string(),
        row["last_timestamp"].as_i64().expect("timestamp"),
    ))
}

/// The ids the vector arm holds for `text`, read back through the real ANN.
async fn vector_hits(harness: &Harness, text: &str) -> Vec<FactId> {
    let embedding = MockEmbedder::new()
        .embed(std::slice::from_ref(&text.to_string()))
        .await
        .expect("embed")
        .remove(0);
    harness
        .vector
        .search(&scope(), &embedding, 10)
        .await
        .expect("vector search")
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// A crash on the truth commit leaves the batch uncommitted: the derived
/// indexes already hold their rows (that is the documented order), but no
/// truth row exists and the ledger did not move — so the retry re-drives the
/// range and lands exactly one fact, and a further retry changes nothing.
#[tokio::test]
async fn fact_commit_then_crash_reconverges() {
    let harness = harness(1, 0).await;
    let turns = vec![turn("t1", TURN_A, 1_000)];

    let crashed = harness.gateway.commit_turns("conv-1", &turns).await;
    assert!(
        crashed.is_err(),
        "a failed truth commit must abort the batch, not swallow the error"
    );

    // Derived first, truth last: the vector row is already there, which is
    // exactly why the watermark is the only thing allowed to say "done".
    assert_eq!(
        vector_hits(&harness, TURN_A).await.len(),
        1,
        "derived indexes are written before the truth row"
    );
    assert_eq!(
        fact_bytes(harness.facts.as_ref(), &scope()).await,
        "",
        "a crash before the commit leaves no truth row at all"
    );
    assert_eq!(
        watermark(&harness, "conv-1").await,
        None,
        "nothing is marked done while the batch is uncommitted"
    );

    // Recovery: the retry converges on exactly one fact and closes the ledger.
    let recovered = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("replay after the crash");
    assert_eq!(recovered.written, 1);
    let snapshot = fact_bytes(harness.facts.as_ref(), &scope()).await;
    assert_eq!(
        snapshot.lines().count(),
        1,
        "one turn writes one row, not one per attempt: {snapshot}"
    );
    assert_eq!(
        watermark(&harness, "conv-1").await,
        Some(("t1".to_string(), 1_000))
    );

    // Idempotent replay: the byte-level snapshot is unchanged, and the vector
    // arm did not accumulate a second row for the same fact id.
    let replayed = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("second replay");
    assert_eq!(replayed.written, 0, "the watermark already covers the turn");
    assert_eq!(fact_bytes(harness.facts.as_ref(), &scope()).await, snapshot);
    assert_eq!(vector_hits(&harness, TURN_A).await.len(), 1);
    assert_eq!(
        watermark(&harness, "conv-1").await,
        Some(("t1".to_string(), 1_000))
    );
}

/// A crash between the truth commit and the watermark is the direction that
/// must be safe to get wrong: the ledger has to lag the data, because a ledger
/// that ran ahead would make the next run skip a range nothing ever wrote.
#[tokio::test]
async fn crash_between_fact_and_watermark_converges() {
    let harness = harness(0, 1).await;
    let turns = vec![turn("t1", TURN_A, 1_000)];

    let crashed = harness.gateway.commit_turns("conv-1", &turns).await;
    assert!(
        crashed.is_err(),
        "a failed watermark write must surface to the caller"
    );

    let snapshot = fact_bytes(harness.facts.as_ref(), &scope()).await;
    assert_eq!(
        snapshot.lines().count(),
        1,
        "the truth row committed before the watermark: {snapshot}"
    );
    assert_eq!(
        watermark(&harness, "conv-1").await,
        None,
        "the watermark must lag the data, never lead it"
    );

    // Replay: the row is already there, so the turn is recognised as known
    // rather than written twice, and only then does the ledger catch up.
    let recovered = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("replay after the crash");
    assert_eq!(recovered.written, 0, "the row is already committed");
    assert_eq!(
        fact_bytes(harness.facts.as_ref(), &scope()).await,
        snapshot,
        "the replay must not duplicate or rewrite the committed row"
    );
    assert_eq!(
        watermark(&harness, "conv-1").await,
        Some(("t1".to_string(), 1_000)),
        "the ledger catches up to the data on the retry"
    );

    // With the ledger in place the range is covered: `covered()` filters it and
    // a third run touches neither the rows nor the watermark.
    let settled = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("third run");
    assert_eq!(settled.considered, 0);
    assert_eq!(fact_bytes(harness.facts.as_ref(), &scope()).await, snapshot);
    assert_eq!(
        watermark(&harness, "conv-1").await,
        Some(("t1".to_string(), 1_000))
    );
}

/// A crash on a *later* turn must leave the turns already committed byte-for-
/// byte intact, and the ledger exactly where it was: neither regressing to
/// the failed turn nor advancing past it. Delivering the whole range again
/// then commits only the turn that never landed and moves the ledger to the
/// batch's latest turn — the watermark is taken from the max timestamp
/// (`gateway.rs:490`), so an out-of-order tail cannot drag it backwards.
#[tokio::test]
async fn a_later_crash_keeps_the_committed_prefix_intact() {
    let harness = harness(0, 0).await;
    let turns = vec![turn("t1", TURN_A, 1_000), turn("t2", TURN_B, 2_000)];

    // The first turn lands cleanly, so the crash below has a committed prefix
    // whose bytes the retry must not disturb.
    let seeded = harness
        .gateway
        .commit_turns("conv-1", &turns[..1])
        .await
        .expect("seed the first turn");
    assert_eq!(seeded.written, 1);
    let committed = fact_bytes(harness.facts.as_ref(), &scope()).await;
    // Arm the fault only now, so the crash lands on the *next* truth commit
    // rather than on the seeding write.
    harness.crash_next_puts(1);

    assert_eq!(committed.lines().count(), 1);

    // Now deliver the whole range with the truth commit broken: t1 is covered
    // by the ledger, t2 is the one that fails.
    let crashed = harness.gateway.commit_turns("conv-1", &turns).await;
    assert!(crashed.is_err(), "the new turn's commit fails");
    assert_eq!(
        fact_bytes(harness.facts.as_ref(), &scope()).await,
        committed,
        "the crash must not disturb the turn already committed"
    );
    assert_eq!(
        watermark(&harness, "conv-1").await,
        Some(("t1".to_string(), 1_000)),
        "the ledger stays on the last committed turn, never on the failed one"
    );

    let recovered = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("replay after the crash");
    assert_eq!(recovered.written, 1, "only the uncommitted turn is written");
    let snapshot = fact_bytes(harness.facts.as_ref(), &scope()).await;
    assert_eq!(snapshot.lines().count(), 2);
    assert!(
        committed
            .lines()
            .all(|row| snapshot.lines().any(|kept| kept == row)),
        "the committed prefix is replayed byte-for-byte, not rewritten"
    );
    assert_eq!(
        watermark(&harness, "conv-1").await,
        Some(("t2".to_string(), 2_000)),
        "the ledger advances to the latest turn of the range"
    );
}
