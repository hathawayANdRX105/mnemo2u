//! Recall orchestrator contract (stage doc 03 T5/T6/T7, AC1–AC5): fixed arm
//! order, degradation marking, arm-by-arm ablation, cross-scope negation,
//! determinism, L0 materialization, and the ten-thousand-row latency
//! baseline.
//!
//! Fixtures run against the three real arm backends — LanceDB vectors, Turso
//! FTS, Turso adjacency — over one `MemoryFactStore` truth layer, because a
//! recall contract that only holds against mocks is a contract about the
//! mocks. Fault injection uses the `Failing*` decorators below, which turn
//! *every* method of one trait into `StoreError::Backend`.

use std::collections::BTreeSet;
use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use lancedb::connect;
use mnemo2u::core::traits::{
    Embedder, FactEdgeStore, FactStore, FactVectorStore, LexicalStore, Result, StoreError,
};
use mnemo2u::core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
use mnemo2u::llm::envelope::{Op, ProviderError};
use mnemo2u::llm::judge::{ExistsVerdict, JudgeClient, JudgeError};
use mnemo2u::llm::judge_mock::{JudgeMock, JudgeOp, JudgeReply, ScriptedReply};
use mnemo2u::llm::mock::MockEmbedder;
use mnemo2u::query::facts::{L0Candidate, MemoryRecall, RecallOptions, StaleRule};
use mnemo2u::store::facts::MemoryFactStore;
use mnemo2u::store::lance_facts::LanceFactVector;
use mnemo2u::store::turso_edges::TursoFactEdges;
use mnemo2u::store::turso_lexical::TursoLexical;

/// Hand-built fixtures use `dim = 8`, matching the vector-arm contract test.
const DIM: usize = 8;

// --- Fixture helpers. ---

fn scope(ws: &str, sess: Option<&str>, task: Option<&str>) -> ScopeKey {
    ScopeKey {
        workspace_id: ws.to_string(),
        session_id: sess.map(str::to_string),
        task_id: task.map(str::to_string),
    }
}

fn id(value: &str) -> FactId {
    FactId(value.to_string())
}

/// The evidence pointer every L0 candidate carries back to the transcript.
fn source_ref() -> SourceRef {
    SourceRef {
        workspace_id: "ws-a".to_string(),
        session_id: "s1".to_string(),
        branch_id: "main".to_string(),
        epoch: 7,
        event_id: "ev-1".to_string(),
    }
}

fn fact(id: &str, scope: &ScopeKey, text: &str) -> Fact {
    Fact {
        id: FactId(id.to_string()),
        scope: scope.clone(),
        kind: "semantic".to_string(),
        text: text.to_string(),
        source_refs: vec![source_ref()],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: 0,
        superseded_by: None,
        revision: 0,
    }
}

/// A fact pinned on the bi-temporal axis: `valid_from` when the fact held in
/// the world, `created_at` when the system learned it.
fn timed_fact(
    id: &str,
    scope: &ScopeKey,
    text: &str,
    valid_from: Option<i64>,
    created_at: i64,
) -> Fact {
    Fact {
        valid_from,
        created_at,
        ..fact(id, scope, text)
    }
}

fn edge(src: &str, dst: &str, rel: RelKind, weight: f64, event: &str) -> Edge {
    Edge {
        src: id(src),
        dst: id(dst),
        rel,
        weight,
        source_event_id: event.to_string(),
    }
}

fn opts(top_k: usize, rrf_k: u32, per_arm_limit: usize) -> RecallOptions {
    RecallOptions {
        top_k,
        rrf_k,
        per_arm_limit,
        staleness: None,
    }
}

/// R5.AC6: staleness is derived at read time — an old, under-corroborated
/// row is withheld under a rule; a second independent session revalidates it
/// with no storage write; and a `valid_to`-closed row never reaches L0 even
/// when an arm finds it.
#[tokio::test]
async fn ac6_staleness_is_read_time_and_revalidates_on_corroboration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);

    let mut old = fact("f-old", &ws_a, "alpha beta gamma");
    old.created_at = 1_000;
    stores.facts.put(&old).await.expect("put old");
    let mut fresh = fact("f-fresh", &ws_a, "alpha beta gamma");
    fresh.created_at = 99_000;
    stores.facts.put(&fresh).await.expect("put fresh");
    let mut closed = fact("f-closed", &ws_a, "alpha beta gamma");
    closed.valid_to = Some(5_000);
    stores.facts.put(&closed).await.expect("put closed");
    for fact_id in ["f-old", "f-fresh", "f-closed"] {
        stores
            .vectors
            .upsert(&ws_a, &id(fact_id), &vec8(fact_id))
            .await
            .expect("upsert");
        stores
            .lexical
            .index(&ws_a, &id(fact_id), "alpha beta gamma")
            .await
            .expect("index");
    }

    let recall = stores.all_arms();
    let base = opts(20, 60, 20);
    let stale_opts = RecallOptions {
        staleness: Some(StaleRule {
            max_age_secs: 10_000,
            min_sessions: 2,
            now_secs: 100_000,
        }),
        ..base.clone()
    };

    // Without a rule: current rows pass; the closed row never does, even
    // though both arms still rank it (the read-side half of `is_current`).
    let none = recall
        .recall(&ws_a, "alpha beta gamma", Some(&vec8("query")), &base)
        .await
        .expect("recall");
    let got = BTreeSet::from_iter(ids(&none.candidates));
    assert!(got.contains("f-old") && got.contains("f-fresh"));
    assert!(
        !got.contains("f-closed"),
        "a valid_to-closed row reached L0 via the arms"
    );

    // With the rule: the old single-session row is withheld.
    let filtered = recall
        .recall(&ws_a, "alpha beta gamma", Some(&vec8("query")), &stale_opts)
        .await
        .expect("recall");
    let got = BTreeSet::from_iter(ids(&filtered.candidates));
    assert!(
        !got.contains("f-old"),
        "an under-corroborated stale row reached L0"
    );
    assert!(got.contains("f-fresh"), "a fresh row must pass the rule");

    // Revalidate: a second independent session on the same row — no state
    // transition, staleness simply stops being derivable.
    let mut corroborated = old.clone();
    corroborated.source_refs.push(SourceRef {
        session_id: "s2".to_string(),
        ..source_ref()
    });
    stores
        .facts
        .put(&corroborated)
        .await
        .expect("put corroborated");
    let revalidated = recall
        .recall(&ws_a, "alpha beta gamma", Some(&vec8("query")), &stale_opts)
        .await
        .expect("recall");
    let got = BTreeSet::from_iter(ids(&revalidated.candidates));
    assert!(
        got.contains("f-old"),
        "corroboration must revalidate the stale row"
    );
}

/// Deterministic 8-dim embedding for `seed`: eight bytes of its md5 spread
/// over `(0, 1]`. Stable across runs (`std`'s hashers are not) and never the
/// zero vector, which cosine cannot rank.
fn vec8(seed: &str) -> [f32; DIM] {
    let digest = mnemo2u::core::text::md5_hex(seed);
    let mut out = [0f32; DIM];
    for (slot, chunk) in out.iter_mut().zip(digest.as_bytes().chunks(2)) {
        let byte = u8::from_str_radix(core::str::from_utf8(chunk).expect("hex digest"), 16)
            .expect("hex digit");
        *slot = f32::from(byte) + 1.0f32;
    }
    out
}

/// Candidate ids in returned order.
fn ids(candidates: &[L0Candidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.fact_id.0.clone())
        .collect()
}

/// The `degraded` vocabulary as strings — the arm names are the observable,
/// not the enum layout.
fn degraded_names(outcome: &mnemo2u::query::facts::RecallOutcome) -> Vec<String> {
    outcome.degraded.iter().map(|arm| arm.to_string()).collect()
}

/// The `via` annotation of one candidate, as arm names.
fn via_of(outcome: &mnemo2u::query::facts::RecallOutcome, fact_id: &str) -> Vec<String> {
    outcome
        .candidates
        .iter()
        .find(|candidate| candidate.fact_id.0 == fact_id)
        .map(|candidate| candidate.via.iter().map(|arm| arm.to_string()).collect())
        .unwrap_or_default()
}

// --- Fault-injection decorators: every method of one trait fails. ---

fn injected(arm: &str) -> StoreError {
    StoreError::Backend(format!("injected {arm} arm failure"))
}

struct FailingVector;

#[async_trait]
impl FactVectorStore for FailingVector {
    async fn upsert(&self, _scope: &ScopeKey, _id: &FactId, _embedding: &[f32]) -> Result<()> {
        Err(injected("vector"))
    }
    async fn remove(&self, _scope: &ScopeKey, _id: &FactId) -> Result<()> {
        Err(injected("vector"))
    }
    async fn search(
        &self,
        _scope: &ScopeKey,
        _query: &[f32],
        _limit: usize,
    ) -> Result<Vec<(FactId, f64)>> {
        Err(injected("vector"))
    }
}

struct FailingLexical;

#[async_trait]
impl LexicalStore for FailingLexical {
    async fn index(&self, _scope: &ScopeKey, _id: &FactId, _text: &str) -> Result<()> {
        Err(injected("lexical"))
    }
    async fn drop(&self, _scope: &ScopeKey, _id: &FactId) -> Result<()> {
        Err(injected("lexical"))
    }
    async fn search(
        &self,
        _scope: &ScopeKey,
        _query: &str,
        _top_k: usize,
    ) -> Result<Vec<(FactId, f64)>> {
        Err(injected("lexical"))
    }
}

struct FailingEdges;

#[async_trait]
impl FactEdgeStore for FailingEdges {
    async fn upsert_edges(&self, _scope: &ScopeKey, _edges: &[Edge]) -> Result<()> {
        Err(injected("graph"))
    }
    async fn remove_edges_from(&self, _scope: &ScopeKey, _src: &FactId) -> Result<()> {
        Err(injected("graph"))
    }
    async fn neighbors(
        &self,
        _seeds: &[FactId],
        _hops: u8,
        _scope: &ScopeKey,
    ) -> Result<Vec<Edge>> {
        Err(injected("graph"))
    }
    async fn edges_by_rel(&self, _scope: &ScopeKey, _rel: RelKind) -> Result<Vec<Edge>> {
        Err(injected("graph"))
    }
}

/// The three-arm stores one fixture loads: a truth layer, a Lance table, an
/// FTS database and an adjacency database, shared by reference. `all_arms`
/// wires a full recall; the individual fields back the reduced configurations.
struct Arms {
    facts: Arc<dyn FactStore>,
    vectors: Arc<dyn FactVectorStore>,
    lexical: Arc<dyn LexicalStore>,
    edges: Arc<dyn FactEdgeStore>,
}

async fn arms(dir: &std::path::Path) -> Arms {
    let vectors: Arc<dyn FactVectorStore> = Arc::new(
        LanceFactVector::new(dir.join("vectors").to_str().expect("path"), DIM)
            .await
            .expect("open vector table"),
    );
    let lexical: Arc<dyn LexicalStore> = Arc::new(
        TursoLexical::new(dir.join("lexical.db").to_str().expect("path"))
            .await
            .expect("open lexical db"),
    );
    let edges: Arc<dyn FactEdgeStore> = Arc::new(
        TursoFactEdges::new(dir.join("edges.db").to_str().expect("path"))
            .await
            .expect("open edge db"),
    );
    let facts: Arc<dyn FactStore> = Arc::new(MemoryFactStore::new());
    Arms {
        facts,
        vectors,
        lexical,
        edges,
    }
}

impl Arms {
    /// A recall over all three real arms.
    fn all_arms(&self) -> MemoryRecall {
        MemoryRecall {
            facts: Arc::clone(&self.facts),
            vector: Some(Arc::clone(&self.vectors)),
            lexical: Some(Arc::clone(&self.lexical)),
            edges: Some(Arc::clone(&self.edges)),
            judge: None,
        }
    }
}

/// The fact-vector table's layout, mirroring the vector arm's own schema.
fn vector_table_schema(dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("ws", DataType::Utf8, false),
        Field::new("sess", DataType::Utf8, false),
        Field::new("task", DataType::Utf8, false),
        Field::new("fact_id", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim as i32,
            ),
            true,
        ),
    ]))
}

/// Load `rows` fact vectors into `path` in one batch.
///
/// The arm's own `upsert` is a single-row write: on this host a merge-insert
/// costs about half a second, so the latency fixture cannot be built through
/// it. It writes the same table the arm reads — the arm is still what serves
/// every recall below, and the arm's `upsert` (used by the other tests here)
/// still merges into it.
async fn bulk_seed_vectors(path: &std::path::Path, scope: &ScopeKey, rows: usize) {
    let schema = vector_table_schema(DIM);
    let mut ws = StringBuilder::new();
    let mut sess = StringBuilder::new();
    let mut task = StringBuilder::new();
    let mut fact_id = StringBuilder::new();
    let mut vector = FixedSizeListBuilder::new(Float32Builder::new(), DIM as i32);
    for index in 0..rows {
        let name = format!("f-{index:05}");
        ws.append_value(&scope.workspace_id);
        sess.append_value(scope.session_id.as_deref().unwrap_or_default());
        task.append_value(scope.task_id.as_deref().unwrap_or_default());
        fact_id.append_value(&name);
        for value in vec8(&name) {
            vector.values().append_value(value);
        }
        vector.append(true);
    }
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ws.finish()),
            Arc::new(sess.finish()),
            Arc::new(task.finish()),
            Arc::new(fact_id.finish()),
            Arc::new(vector.finish()),
        ],
    )
    .expect("bulk record batch");
    let path_str = path.to_str().expect("path");
    connect(path_str)
        .execute()
        .await
        .expect("open lancedb")
        .create_empty_table("fact_vectors", schema)
        .execute()
        .await
        .expect("create fact_vectors")
        .add(vec![batch])
        .execute()
        .await
        .expect("bulk load");
}

// --- 1. AC3: single-arm failure degrades, an empty arm does not. ---

/// Vector arm fails: the call still succeeds, names the degraded arm, and the
/// lexical arm's own id survives. Three healthy arms name nothing; an arm
/// that answers with zero hits names nothing either.
#[tokio::test]
async fn ac3_degraded_arm_is_named_and_empty_arms_are_not_degraded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);

    let rows = [
        ("f-hit", "kelpite shelf quantum zebra 9177"),
        ("f-spoke", "marble quarry plumbite amphora 4421"),
    ];
    for (fact_id, text) in rows {
        stores
            .facts
            .put(&fact(fact_id, &ws_a, text))
            .await
            .expect("put");
        stores
            .lexical
            .index(&ws_a, &id(fact_id), text)
            .await
            .expect("index");
        stores
            .vectors
            .upsert(&ws_a, &id(fact_id), &vec8(fact_id))
            .await
            .expect("upsert");
    }
    stores
        .edges
        .upsert_edges(
            &ws_a,
            &[edge("f-hit", "f-spoke", RelKind::Related, 1.0, "ev-1")],
        )
        .await
        .expect("edges");

    // (a) Vector arm injected failure.
    let degraded_vector = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: Some(Arc::new(FailingVector)),
        lexical: Some(Arc::clone(&stores.lexical)),
        edges: Some(Arc::clone(&stores.edges)),
        judge: None,
    };
    let outcome = degraded_vector
        .recall(
            &ws_a,
            "kelpite quantum zebra",
            Some(&vec8("query")),
            &opts(10, 60, 20),
        )
        .await
        .expect("recall survives a failed vector arm");
    assert_eq!(degraded_names(&outcome), vec!["vector".to_string()]);
    let got = ids(&outcome.candidates);
    // The lexical arm's hit is still there, and the graph arm still expanded
    // from it: the failure degrades one arm, not the recall.
    assert!(
        got.contains(&"f-hit".to_string()),
        "lexical id lost: {got:?}"
    );
    assert!(
        got.contains(&"f-spoke".to_string()),
        "graph id lost: {got:?}"
    );

    // (b) Three healthy arms: nothing is degraded.
    let healthy = stores.all_arms();
    let outcome = healthy
        .recall(
            &ws_a,
            "kelpite quantum zebra",
            Some(&vec8("query")),
            &opts(10, 60, 20),
        )
        .await
        .expect("recall");
    assert!(outcome.exists);
    assert!(
        outcome.degraded.is_empty(),
        "healthy arms degraded: {:?}",
        degraded_names(&outcome)
    );
    assert_eq!(
        BTreeSet::from_iter(ids(&outcome.candidates)),
        BTreeSet::from([String::from("f-hit"), String::from("f-spoke")])
    );

    // (c) A lexical arm with zero hits is a normal empty, not a failure.
    let empty = healthy
        .recall(
            &ws_a,
            "xylophone zinnwald quetzal nowhere",
            Some(&vec8("query")),
            &opts(10, 60, 20),
        )
        .await
        .expect("recall");
    assert!(
        empty.degraded.is_empty(),
        "an arm with no hits must not be degraded: {:?}",
        degraded_names(&empty)
    );
}

/// Every configured arm failing is still an `Ok` outcome — empty, degraded,
/// and explicitly "nothing exists".
#[tokio::test]
async fn ac3_all_arms_failed_is_ok_with_exists_false() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);
    stores
        .facts
        .put(&fact("f-1", &ws_a, "kelpite shelf quantum zebra"))
        .await
        .expect("put");

    let broken = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: Some(Arc::new(FailingVector)),
        lexical: Some(Arc::new(FailingLexical)),
        edges: Some(Arc::new(FailingEdges)),
        judge: None,
    };
    let outcome = broken
        .recall(
            &ws_a,
            "kelpite quantum",
            Some(&vec8("query")),
            &opts(10, 60, 20),
        )
        .await
        .expect("every arm failing is still Ok");
    assert!(!outcome.exists);
    assert!(outcome.candidates.is_empty());
    assert_eq!(
        degraded_names(&outcome),
        vec![
            "vector".to_string(),
            "lexical".to_string(),
            "graph".to_string()
        ]
    );
}

// --- 2. AC5: per-arm increment and RRF k sensitivity. ---

/// The lexical arm's own id disappears when the arm is removed, and `rrf_k`
/// is a real dial: the same fixture flips its top two candidates between
/// k = 1 and the k = 60 default.
#[tokio::test]
async fn ac5_lexical_increment_and_rrf_k_dial() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);

    // `f-lex-only` is indexed in the FTS table and has no vector row at all,
    // so only the lexical arm can ever return it.
    stores
        .facts
        .put(&fact("f-lex-only", &ws_a, "quartz shelf mineral samples"))
        .await
        .expect("put");
    stores
        .lexical
        .index(&ws_a, &id("f-lex-only"), "quartz shelf mineral samples")
        .await
        .expect("index");

    let with_both = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: None,
        lexical: Some(Arc::clone(&stores.lexical)),
        edges: None,
        judge: None,
    };
    let outcome = with_both
        .recall(&ws_a, "quartz shelf mineral", None, &opts(10, 60, 20))
        .await
        .expect("recall");
    assert_eq!(
        ids(&outcome.candidates),
        vec![String::from("f-lex-only")],
        "the lexical-only id must be the fused set"
    );
    assert_eq!(via_of(&outcome, "f-lex-only"), vec!["lexical".to_string()]);

    let lexical_removed = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: None,
        lexical: None,
        edges: None,
        judge: None,
    };
    let outcome = lexical_removed
        .recall(&ws_a, "quartz shelf mineral", None, &opts(10, 60, 20))
        .await
        .expect("recall");
    assert!(
        outcome.candidates.is_empty(),
        "the lexical-only id survived without its arm: {:?}",
        ids(&outcome.candidates)
    );
    assert!(!outcome.exists);

    // Ranked for the k dial. The lexical column shares one text (distinct
    // from `f-lex-only`'s, so the incremental subtest above cannot pollute
    // the FTS ranking), so it ties on score and the arm's id tie-break fixes
    // its order: lex-1..lex-6. The cosine column fixes the vector arm's
    // order: lex-6 at rank 1, lex-4 at rank 2, `vec-only` at rank 3.
    let query_vec = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let text = "kelpite shelf plumbite amphora";
    for (fact_id, embedding) in [
        ("lex-6", [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ("lex-4", [0.5, 0.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ("vec-only", [0.25, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ] {
        stores
            .facts
            .put(&fact(fact_id, &ws_a, text))
            .await
            .expect("put");
        stores
            .vectors
            .upsert(&ws_a, &id(fact_id), &embedding)
            .await
            .expect("upsert");
    }
    // `lex-1..3` and `lex-5` carry no vector row, so the vector arm sees
    // exactly the three ids above.
    for filler in ["lex-1", "lex-2", "lex-3", "lex-5"] {
        stores
            .facts
            .put(&fact(filler, &ws_a, text))
            .await
            .expect("put");
        stores
            .lexical
            .index(&ws_a, &id(filler), text)
            .await
            .expect("index");
    }
    for fact_id in ["lex-4", "lex-6"] {
        stores
            .lexical
            .index(&ws_a, &id(fact_id), text)
            .await
            .expect("index");
    }

    let ranked = stores.all_arms();
    let flat = ranked
        .recall(&ws_a, text, Some(&query_vec), &opts(10, 1, 20))
        .await
        .expect("recall k=1");
    let damped = ranked
        .recall(&ws_a, text, Some(&query_vec), &opts(10, 60, 20))
        .await
        .expect("recall k=60");

    // k = 1: `lex-6` = 1/(1+1) + 1/(1+6) = 23/35 beats `lex-4` = 1/(1+2) +
    // 1/(1+4) = 8/15 — a single deep second-arm hit outweighs a shallow one.
    // k = 60: the tighter rank sum wins: `lex-4` = 1/62 + 1/64 > `lex-6` =
    // 1/61 + 1/66. The single-arm id `vec-only` trails both two-arm ids.
    assert_eq!(
        ids(&flat.candidates).first().cloned(),
        Some(String::from("lex-6")),
        "k=1 rewards the top rank: {:?}",
        ids(&flat.candidates)
    );
    assert_eq!(
        ids(&damped.candidates).first().cloned(),
        Some(String::from("lex-4")),
        "k=60 rewards the tighter rank sum: {:?}",
        ids(&damped.candidates)
    );
    let damped_ids = ids(&damped.candidates);
    let single_index = damped_ids
        .iter()
        .position(|got| got == "vec-only")
        .expect("the single-arm id must be fused");
    for two_arm in ["lex-4", "lex-6"] {
        let index = damped_ids
            .iter()
            .position(|got| got == two_arm)
            .expect("two-arm id fused");
        assert!(
            index < single_index,
            "a two-arm id must outrank the single-arm one: {damped_ids:?}"
        );
    }
}
// --- 3. AC2: cross-workspace and session negation across all three arms. ---

/// Workspace B mirrors workspace A — same texts, same structure, and the
/// *better* aligned vectors — so an unscoped arm would rank B first. A
/// ws-a recall must still return only ws-a's ids, and a session-narrowed
/// recall only that session's.
#[tokio::test]
async fn ac2_three_arms_never_cross_a_workspace_or_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);
    let ws_b = scope("ws-b", None, None);
    let s1 = scope("ws-a", Some("s1"), None);
    let s2 = scope("ws-a", Some("s2"), None);

    // ws-b's vectors are perfectly aligned with the query; ws-a's are not.
    let hot = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let warm = [0.25, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let middling = [0.5, 0.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let rows: Vec<(&str, &ScopeKey, &str, [f32; DIM])> = vec![
        (
            "a-1",
            &ws_a,
            "the kelpite shelf holds the staged quantum notes",
            warm,
        ),
        (
            "a-2",
            &ws_a,
            "the kelpite shelf seals the quantum draft",
            warm,
        ),
        (
            "a-3",
            &ws_a,
            "marble quarry backs up the staged claims",
            warm,
        ),
        (
            "a-s1",
            &s1,
            "session one staged the kelpite shelf notes",
            middling,
        ),
        (
            "a-s2",
            &s2,
            "session two staged the kelpite shelf drafts",
            middling,
        ),
        (
            "b-1",
            &ws_b,
            "the kelpite shelf holds the staged quantum notes",
            hot,
        ),
        (
            "b-2",
            &ws_b,
            "the kelpite shelf seals the quantum draft",
            hot,
        ),
        (
            "b-3",
            &ws_b,
            "marble quarry backs up the staged claims",
            hot,
        ),
    ];
    for &(fact_id, fact_scope, text, embedding) in &rows {
        stores
            .facts
            .put(&fact(fact_id, fact_scope, text))
            .await
            .expect("put");
        stores
            .lexical
            .index(fact_scope, &id(fact_id), text)
            .await
            .expect("index");
        stores
            .vectors
            .upsert(fact_scope, &id(fact_id), &embedding)
            .await
            .expect("upsert");
    }
    // Mirror the adjacency too, with heavier relations: a graph arm that
    // ignored the scope would drag B's chain in and score it highest.
    stores
        .edges
        .upsert_edges(
            &ws_a,
            &[
                edge("a-1", "a-2", RelKind::Related, 1.0, "ev-a1"),
                edge("a-2", "a-3", RelKind::DependsOn, 1.0, "ev-a2"),
            ],
        )
        .await
        .expect("edges ws-a");
    stores
        .edges
        .upsert_edges(
            &ws_b,
            &[
                edge("b-1", "b-2", RelKind::Corrects, 2.0, "ev-b1"),
                edge("b-2", "b-3", RelKind::Supersedes, 2.0, "ev-b2"),
            ],
        )
        .await
        .expect("edges ws-b");

    let recall = stores.all_arms();
    let outcome = recall
        .recall(
            &ws_a,
            "kelpite quantum shelf",
            Some(&hot),
            &opts(20, 60, 20),
        )
        .await
        .expect("recall ws-a");
    assert!(outcome.exists);
    let got = BTreeSet::from_iter(ids(&outcome.candidates));
    let leaked: Vec<&String> = got
        .iter()
        .filter(|fact_id| fact_id.starts_with("b-"))
        .collect();
    assert!(leaked.is_empty(), "ws-b leaked into a ws-a recall: {got:?}");
    // L0 materialization is an exact-scope truth read: rows the arms ranked
    // under narrower (session) cells are not readable by the workspace-wide
    // `get`, so only the workspace-celled rows come back.
    assert_eq!(
        got,
        BTreeSet::from([
            String::from("a-1"),
            String::from("a-2"),
            String::from("a-3"),
        ])
    );

    // Session narrowing follows `ScopeKey::admits`: a session-constrained
    // caller sees that session's rows and nothing else.
    let narrowed = recall
        .recall(&s1, "kelpite quantum shelf", Some(&hot), &opts(20, 60, 20))
        .await
        .expect("recall s1");
    let s1_ids = BTreeSet::from_iter(ids(&narrowed.candidates));
    assert_eq!(
        s1_ids,
        BTreeSet::from([String::from("a-s1")]),
        "the s1 recall must see only its own cell"
    );
    let s2_outcome = recall
        .recall(&s2, "kelpite quantum shelf", Some(&hot), &opts(20, 60, 20))
        .await
        .expect("recall s2");
    assert_eq!(
        BTreeSet::from_iter(ids(&s2_outcome.candidates)),
        BTreeSet::from([String::from("a-s2")]),
        "the s2 recall must see only its own cell"
    );
}

// --- 4. AC1: an exact identifier beats its paraphrase only with fusion. ---

/// The mock embedder's bag of words cannot separate `CheckAuthError(401)`
/// from a paraphrase that drops the identifier — a diluted exact hit loses on
/// cosine alone. The lexical arm's phrase match pulls it back to the top.
#[tokio::test]
async fn ac1_exact_identifier_wins_only_with_the_lexical_arm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws_a = scope("ws-a", None, None);
    let facts: Arc<dyn FactStore> = Arc::new(MemoryFactStore::new());

    // The exact identifier is buried in a long body: bag-of-words cosine
    // divides by document length, so the precise row is the *diluted* one.
    let exact = format!(
        "the gateway raised CheckAuthError(401) while verifying the bearer token {}",
        (0..24)
            .map(|index| format!("filler{index}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let paraphrase = "unauthorized 401";
    for (fact_id, text) in [("f-exact", exact.as_str()), ("f-paraphrase", paraphrase)] {
        facts.put(&fact(fact_id, &ws_a, text)).await.expect("put");
    }

    // MockEmbedder is 64-dimensional; the table follows it.
    let embedder = MockEmbedder::new();
    let vectors: Arc<dyn FactVectorStore> = Arc::new(
        LanceFactVector::new(
            dir.path().join("vectors").to_str().expect("path"),
            embedder.dim(),
        )
        .await
        .expect("open"),
    );
    let mut embedded = embedder
        .embed(&[exact.clone(), paraphrase.to_string()])
        .await
        .expect("embed");
    let paraphrase_vector = embedded.pop().expect("second row");
    let exact_vector = embedded.pop().expect("first row");
    vectors
        .upsert(&ws_a, &id("f-exact"), &exact_vector)
        .await
        .expect("upsert");
    vectors
        .upsert(&ws_a, &id("f-paraphrase"), &paraphrase_vector)
        .await
        .expect("upsert");

    let lexical = TursoLexical::new(dir.path().join("lexical.db").to_str().expect("path"))
        .await
        .expect("open lexical db");
    for (fact_id, text) in [("f-exact", exact.as_str()), ("f-paraphrase", paraphrase)] {
        lexical
            .index(&ws_a, &id(fact_id), text)
            .await
            .expect("index");
    }

    let query_text = "CheckAuthError(401)";
    let query_vec = embedder
        .embed(&[query_text.to_string()])
        .await
        .expect("embed query")
        .pop()
        .expect("one vector");

    let vector_only = MemoryRecall {
        facts: Arc::clone(&facts),
        vector: Some(Arc::clone(&vectors)),
        lexical: None,
        edges: None,
        judge: None,
    };
    let alone = vector_only
        .recall(&ws_a, query_text, Some(&query_vec), &opts(10, 60, 20))
        .await
        .expect("vector-only recall");
    let alone_ids = ids(&alone.candidates);
    assert_eq!(alone_ids.len(), 2, "both rows must rank: {alone_ids:?}");
    assert_ne!(
        alone_ids.first().cloned(),
        Some(String::from("f-exact")),
        "the pure vector arm was supposed to miss the exact identifier: {alone_ids:?}"
    );

    let fused = MemoryRecall {
        facts: Arc::clone(&facts),
        vector: Some(Arc::clone(&vectors)),
        lexical: Some(Arc::new(lexical)),
        edges: None,
        judge: None,
    };
    let together = fused
        .recall(&ws_a, query_text, Some(&query_vec), &opts(10, 60, 20))
        .await
        .expect("fused recall");
    assert_eq!(
        ids(&together.candidates).first().cloned(),
        Some(String::from("f-exact")),
        "fusion must promote the exact identifier: {:?}",
        ids(&together.candidates)
    );
    assert_eq!(
        via_of(&together, "f-exact"),
        vec!["vector".to_string(), "lexical".to_string()]
    );
}
// --- 5. T6/T7: determinism, arm provenance, summaries, corrections. ---

/// Two identical recalls return an identical candidate sequence; `via` names
/// the arms that found each id; a long body is cut to exactly 200 chars on a
/// char boundary; a superseded row never reaches L0.
#[tokio::test]
async fn t6_t7_determinism_provenance_summary_and_supersede() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);

    let long_text: String = "é".repeat(300);
    let rows = [
        ("f-long", long_text.as_str()),
        ("f-via-vlg", "alpha beta gamma"),
        ("f-via-vl", "alpha beta gamma"),
        ("f-via-g", "alpha beta gamma"),
        ("f-pair", "alpha beta gamma"),
        ("f-succ", "alpha beta gamma"),
    ];
    for (fact_id, text) in rows {
        stores
            .facts
            .put(&fact(fact_id, &ws_a, text))
            .await
            .expect("put");
    }
    // Supersede before anything else reads the store: `f-pair` keeps its row
    // but is no longer the truth.
    stores
        .facts
        .supersede(
            &id("f-pair"),
            &fact("f-succ", &ws_a, "alpha beta gamma corrected"),
        )
        .await
        .expect("supersede");

    // Vector rows for the vector+lexical ids, plus the stale row that must
    // never reach L0.
    for fact_id in ["f-long", "f-via-vlg", "f-via-vl", "f-pair"] {
        stores
            .vectors
            .upsert(&ws_a, &id(fact_id), &vec8(fact_id))
            .await
            .expect("upsert");
    }
    for fact_id in ["f-long", "f-via-vlg", "f-via-vl", "f-pair"] {
        stores
            .lexical
            .index(&ws_a, &id(fact_id), "alpha beta gamma delta")
            .await
            .expect("index");
    }
    // `f-via-g` is graph-only; `f-succ` is reached from the stale `f-pair`.
    stores
        .edges
        .upsert_edges(
            &ws_a,
            &[
                edge("f-via-vlg", "f-via-g", RelKind::Corrects, 1.5, "ev-1"),
                edge("f-succ", "f-pair", RelKind::Supersedes, 1.0, "ev-2"),
            ],
        )
        .await
        .expect("edges");

    let recall = stores.all_arms();
    let options = opts(20, 60, 20);
    let first = recall
        .recall(&ws_a, "alpha beta gamma", Some(&vec8("query")), &options)
        .await
        .expect("recall");
    let second = recall
        .recall(&ws_a, "alpha beta gamma", Some(&vec8("query")), &options)
        .await
        .expect("recall");

    assert_eq!(
        ids(&first.candidates),
        ids(&second.candidates),
        "the same recall must return the same sequence"
    );
    let scores: Vec<f64> = first
        .candidates
        .iter()
        .map(|candidate| candidate.score)
        .collect();
    let second_scores: Vec<f64> = second
        .candidates
        .iter()
        .map(|candidate| candidate.score)
        .collect();
    assert_eq!(scores, second_scores, "scores must be deterministic");

    let got = BTreeSet::from_iter(ids(&first.candidates));
    assert!(!got.contains("f-pair"), "a superseded row reached L0");
    assert!(got.contains("f-succ"), "the correction did not reach L0");
    assert!(got.contains("f-long"));
    assert!(first.exists);
    assert!(first.degraded.is_empty());

    // Provenance: all three arms for the seeded row, two for its
    // vector+lexical neighbour, and the graph arm alone for the node only it
    // reached.
    assert_eq!(
        via_of(&first, "f-via-vlg"),
        vec![
            "vector".to_string(),
            "lexical".to_string(),
            "graph".to_string()
        ]
    );
    assert_eq!(
        via_of(&first, "f-via-vl"),
        vec!["vector".to_string(), "lexical".to_string()]
    );
    assert_eq!(
        via_of(&first, "f-via-g"),
        vec!["graph".to_string()],
        "the temporal arm never authors an id, so a graph-only hit names one arm"
    );

    // Summary: exactly 200 chars, cut on a char boundary, refs and trust
    // intact.
    let summary = first
        .candidates
        .iter()
        .find(|candidate| candidate.fact_id.0 == "f-long")
        .expect("f-long candidate")
        .summary
        .clone();
    assert_eq!(summary.chars().count(), 200);
    assert_eq!(summary, long_text.chars().take(200).collect::<String>());
    assert!(summary.is_char_boundary(summary.len()));
    let candidate = first
        .candidates
        .iter()
        .find(|candidate| candidate.fact_id.0 == "f-via-vl")
        .expect("candidate");
    assert_eq!(candidate.source_refs, vec![source_ref()]);
    assert_eq!(candidate.trust_state, TrustState::Verified);
}

/// `exists` is false — with an empty candidate list — when no arm produced a
/// hit at all: an absent vector store, a lexical arm with no match, and no
/// seed for the graph arm. Nothing degraded: nothing failed.
#[tokio::test]
async fn t7_exists_false_yields_no_candidates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);
    stores
        .facts
        .put(&fact("f-1", &ws_a, "kelpite shelf quantum zebra"))
        .await
        .expect("put");
    stores
        .lexical
        .index(&ws_a, &id("f-1"), "kelpite shelf quantum zebra")
        .await
        .expect("index");

    let recall = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: None,
        lexical: Some(Arc::clone(&stores.lexical)),
        edges: Some(Arc::clone(&stores.edges)),
        judge: None,
    };
    let outcome = recall
        .recall(&ws_a, "xylophone zinnwald quetzal", None, &opts(20, 60, 20))
        .await
        .expect("recall");
    assert!(!outcome.exists);
    assert!(outcome.candidates.is_empty());
    assert!(outcome.degraded.is_empty());
}

/// The temporal arm re-orders the fused set and never widens it: with a date
/// in the query the fact closest to that date leads; without one the RRF
/// order stands.
#[tokio::test]
async fn t6_temporal_arm_reorders_without_injecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores = arms(dir.path()).await;
    let ws_a = scope("ws-a", None, None);

    const DAY_MS: i64 = 86_400_000;
    let centre = 20_468 * DAY_MS; // 2026-01-15 is 20468 days past the epoch
    let rows = [
        ("f-close", centre),
        ("f-far", centre - 40 * DAY_MS),
        ("f-window", centre + 6 * DAY_MS),
    ];
    for (fact_id, valid_from) in rows {
        stores
            .facts
            .put(&timed_fact(
                fact_id,
                &ws_a,
                "quartz shelf mineral samples 2026-01-15",
                Some(valid_from),
                0,
            ))
            .await
            .expect("put");
        stores
            .lexical
            .index(
                &ws_a,
                &id(fact_id),
                "quartz shelf mineral samples 2026-01-15",
            )
            .await
            .expect("index");
    }
    let recall = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: None,
        lexical: Some(Arc::clone(&stores.lexical)),
        edges: None,
        judge: None,
    };

    // Identical lexical rows tie on score, so the fused order is the fact_id
    // tie-break — `f-close` leads without any time expression.
    let plain = recall
        .recall(
            &ws_a,
            "quartz shelf mineral samples",
            None,
            &opts(20, 60, 20),
        )
        .await
        .expect("recall without a date");
    assert_eq!(
        ids(&plain.candidates).first().cloned(),
        Some(String::from("f-close"))
    );

    // 2026-01-15 sits at `centre` above; the query carries that date, so
    // proximity takes over: `f-close` is the day itself, `f-window` a week
    // out, `f-far` forty days out.
    let dated = recall
        .recall(
            &ws_a,
            "quartz shelf mineral samples 2026-01-15",
            None,
            &opts(20, 60, 20),
        )
        .await
        .expect("recall with a date");
    assert_eq!(
        ids(&dated.candidates),
        vec![
            String::from("f-close"),
            String::from("f-window"),
            String::from("f-far"),
        ],
        "proximity re-ordering failed"
    );
    assert_eq!(
        BTreeSet::from_iter(ids(&dated.candidates)),
        BTreeSet::from_iter(ids(&plain.candidates)),
        "the temporal arm must not inject candidates"
    );
}

// --- 6. AC4: latency baseline over ten thousand rows. ---

/// The hot-path budget (stage doc 03 contract 5): three local arms over ten
/// thousand facts, 101 recalls, P50/P95 printed as the regression baseline.
#[tokio::test]
async fn ac4_ten_thousand_row_recall_latency() {
    const ROWS: usize = 10_000;
    const RUNS: usize = 101;

    let dir = tempfile::tempdir().expect("tempdir");
    let ws_a = scope("ws-a", None, None);
    // The vector arm's table is loaded in one batch instead of ten thousand
    // single-row `upsert`s: the arm's write path costs about half a second
    // per row on this host, so a ten-thousand-row fixture cannot be built
    // through it. The batch writes the table the arm reads — every recall
    // below goes through the arm's own `search`.
    bulk_seed_vectors(&dir.path().join("vectors"), &ws_a, ROWS).await;
    let stores = arms(dir.path()).await;
    // A realistic corpus is diverse: only the first 500 rows carry the query
    // tokens, so the FTS arm ranks a small match set instead of the whole
    // ten thousand rows.
    for index in 0..ROWS {
        let text = if index < 500 {
            format!("plumite quarry kelpite shelf staging area {index} holds the staged notes")
        } else {
            format!(
                "amphora {} sigil draft {} backs the cast claims",
                index % 97,
                index
            )
        };
        let fact_id = format!("f-{index:05}");
        stores
            .facts
            .put(&fact(&fact_id, &ws_a, &text))
            .await
            .expect("put");
        stores
            .lexical
            .index(&ws_a, &id(&fact_id), &text)
            .await
            .expect("index");
    }
    // A seed-and-spoke chain: the graph arm expands from the top-10 seeds.
    for index in 0..200 {
        stores
            .edges
            .upsert_edges(
                &ws_a,
                &[edge(
                    &format!("f-{index:05}"),
                    &format!("f-{:05}", index + 1),
                    RelKind::Related,
                    1.0,
                    "ev-bulk",
                )],
            )
            .await
            .expect("edges");
    }

    let recall = stores.all_arms();
    let options = opts(50, 60, 250);

    let query_vec = vec8("query-anchor");
    let query_text = "plumite quarry kelpite shelf";
    let mut samples: Vec<u128> = Vec::with_capacity(RUNS);
    let mut checked = 0usize;
    for _ in 0..RUNS {
        let started = std::time::Instant::now();
        let outcome = recall
            .recall(&ws_a, query_text, Some(&query_vec), &options)
            .await
            .expect("recall");
        samples.push(started.elapsed().as_micros());
        assert!(outcome.exists, "the ten-thousand-row corpus must recall");
        assert!(
            outcome.candidates.len() <= options.top_k,
            "the budget held: {} candidates",
            outcome.candidates.len()
        );
        checked += outcome.candidates.len();
    }
    assert!(checked > 0, "every run returned candidates");
    samples.sort_unstable();
    let p50 = samples[RUNS / 2];
    let p95 = samples[(RUNS * 95) / 100];
    println!(
        "AC4 recall latency over {ROWS} rows, {RUNS} runs: P50={}ms P95={}ms",
        p50 / 1_000,
        p95 / 1_000
    );
    assert!(
        p95 < 500_000,
        "P95 = {}µs exceeds the 500ms local budget",
        p95
    );
}

// --- R4.T4: the judgment stage (rerank + exists verdict). ---

/// Seeds two lexical-only facts and returns a recall over them with
/// the given judge attached.
async fn judged_recall(
    dir: &std::path::Path,
    judge: Arc<dyn JudgeClient>,
) -> (MemoryRecall, ScopeKey) {
    let stores = arms(dir).await;
    let ws_a = scope("ws-a", None, None);
    let rows = [
        ("f-hit", "kelpite shelf quantum zebra 9177"),
        ("f-spoke", "kelpite marble quarry plumbite 4421"),
    ];
    for (fact_id, text) in rows {
        stores
            .facts
            .put(&fact(fact_id, &ws_a, text))
            .await
            .expect("put");
        stores
            .lexical
            .index(&ws_a, &id(fact_id), text)
            .await
            .expect("index");
    }
    let recall = MemoryRecall {
        facts: Arc::clone(&stores.facts),
        vector: None,
        lexical: Some(Arc::clone(&stores.lexical)),
        edges: None,
        judge: Some(judge),
    };
    (recall, ws_a)
}

/// Rerank re-orders the fused set by judged relevance; an
/// `Answered` verdict keeps the recall alive.
#[tokio::test]
async fn judge_rerank_reorders_and_answered_keeps_the_recall() {
    let dir = tempfile::tempdir().expect("tempdir");
    let judge = Arc::new(JudgeMock::new(
        "judge-mock",
        vec![
            JudgeMock::rerank_ranked(&[("f-spoke", 0.9), ("f-hit", 0.4)]),
            JudgeMock::find(Some(("f-spoke", 0.9)), ExistsVerdict::Answered),
        ],
    ));
    let (recall, ws_a) = judged_recall(dir.path(), judge).await;
    let outcome = recall
        .recall(&ws_a, "kelpite", None, &opts(10, 60, 20))
        .await
        .expect("recall");

    assert!(outcome.exists);
    assert_eq!(ids(&outcome.candidates), vec!["f-spoke", "f-hit"]);
    assert!(degraded_names(&outcome).is_empty());
}

/// A whole near-zero rerank column is a miss, not a confident
/// ordering (R4 contract).
#[tokio::test]
async fn judge_near_zero_column_is_a_miss() {
    let dir = tempfile::tempdir().expect("tempdir");
    let judge = Arc::new(JudgeMock::new(
        "judge-mock",
        vec![JudgeMock::rerank_ranked(&[
            ("f-hit", 0.0),
            ("f-spoke", 0.05),
        ])],
    ));
    let (recall, ws_a) = judged_recall(dir.path(), judge).await;
    let outcome = recall
        .recall(&ws_a, "kelpite", None, &opts(10, 60, 20))
        .await
        .expect("recall");

    assert!(!outcome.exists, "a near-zero column is a miss");
    assert!(outcome.candidates.is_empty());
}

/// `find`'s `Absent` verdict clears the recall even when the
/// rerank found relevance; no best is forced on an absent set.
#[tokio::test]
async fn judge_find_absent_clears_the_recall() {
    let dir = tempfile::tempdir().expect("tempdir");
    let judge = Arc::new(JudgeMock::new(
        "judge-mock",
        vec![
            JudgeMock::rerank_ranked(&[("f-hit", 0.9), ("f-spoke", 0.4)]),
            JudgeMock::find(None, ExistsVerdict::Absent),
        ],
    ));
    let (recall, ws_a) = judged_recall(dir.path(), judge).await;
    let outcome = recall
        .recall(&ws_a, "kelpite", None, &opts(10, 60, 20))
        .await
        .expect("recall");

    assert!(!outcome.exists);
    assert!(outcome.candidates.is_empty());
}

/// A failed judge call degrades: the fused order stands and `Judge`
/// names the stage — recall still succeeds.
#[tokio::test]
async fn judge_failure_degrades_to_the_fused_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let judge = Arc::new(JudgeMock::new(
        "judge-mock",
        vec![ScriptedReply {
            op: JudgeOp::Rerank,
            reply: JudgeReply::Fail(JudgeError::Backend(ProviderError::Unavailable {
                op: Op::Judge,
            })),
        }],
    ));
    let (recall, ws_a) = judged_recall(dir.path(), judge).await;
    let outcome = recall
        .recall(&ws_a, "kelpite", None, &opts(10, 60, 20))
        .await
        .expect("recall survives a failed judge call");

    assert!(outcome.exists);
    assert_eq!(
        BTreeSet::from_iter(ids(&outcome.candidates)),
        BTreeSet::from_iter(vec![String::from("f-hit"), String::from("f-spoke")]),
        "the fused order stands"
    );
    assert_eq!(degraded_names(&outcome), vec!["judge"]);
}
