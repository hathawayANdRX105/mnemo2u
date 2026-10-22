//! Write-gateway integration tests — closed turns → facts, over the real
//! backends (turso facts/lexical/edges, lancedb vectors, an in-memory
//! watermark), with a scripted judge.

use std::sync::Arc;

use mnemo2u::core::text::Tokenizer;
use mnemo2u::core::traits::{
    Embedder, FactEdgeStore, FactStore, FactVectorStore, KvStore, LexicalStore,
};
use mnemo2u::index::cost::CostOptions;
use mnemo2u::index::gateway::{FactWriteGateway, GatewayOptions};
use mnemo2u::index::ingest::{SessionTurn, Turn};
use mnemo2u::llm::judge::{
    CompareRelation, CompareResponse, ExtractResponse, ExtractResult, ExtractStatus, JudgeError,
    Verdict,
};
use mnemo2u::llm::judge_mock::{JudgeMock, JudgeReply, ScriptedReply};
use mnemo2u::store::lance_facts::LanceFactVector;
use mnemo2u::store::memory::MemoryKv;
use mnemo2u::store::turso_edges::TursoFactEdges;
use mnemo2u::store::turso_facts::TursoFactStore;
use mnemo2u::store::turso_lexical::TursoLexical;
use mnemo2u::{FactId, ScopeKey};

/// The mock embedder's dimension.
const DIM: usize = 64;

/// The scope every test writes under.
fn scope() -> ScopeKey {
    ScopeKey {
        workspace_id: "ws-a".to_string(),
        session_id: None,
        task_id: None,
    }
}

/// Options with the value gates open, so the fixtures exercise the judge and
/// commit paths rather than the thresholds.
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

/// One closed turn.
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

/// One closed turn of a named conversation (the source-ref session falls
/// back to the conversation id when the scope carries none).
fn turn_in(conversation_id: &str, turn_id: &str, content: &str, timestamp: i64) -> SessionTurn {
    SessionTurn {
        conversation_id: conversation_id.to_string(),
        turn: Turn {
            turn_id: turn_id.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            timestamp,
        },
    }
}

/// A real durable gateway over temp files.
struct Harness {
    _dir: tempfile::TempDir,
    gateway: FactWriteGateway,
    facts: Arc<TursoFactStore>,
    vector: Arc<LanceFactVector>,
    lexical: Arc<TursoLexical>,
    edges: Arc<TursoFactEdges>,
    watermarks: Arc<MemoryKv>,
    tombstones: Arc<MemoryKv>,
    judge: Arc<JudgeMock>,
}

/// Build a gateway with all four stores wired and a scripted judge.
///
/// `options` is taken verbatim, so a test that wants the production
/// screening defaults passes them and a test that wants the gates open
/// passes [`gate_open`].
async fn harness(script: Vec<ScriptedReply>, options: GatewayOptions) -> Harness {
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
    let watermarks = Arc::new(MemoryKv::new());
    let tombstones = Arc::new(MemoryKv::new());
    let judge = Arc::new(JudgeMock::new("judge-mock", script));

    let gateway = FactWriteGateway::new(
        options,
        mnemo2u::index::gateway::GatewayParts {
            facts: facts.clone(),
            vector: Some(vector.clone()),
            lexical: Some(lexical.clone()),
            edges: Some(edges.clone()),
            embedder: Arc::new(mnemo2u::llm::mock::MockEmbedder::new()),
            judge: Some(judge.clone()),
            watermarks: watermarks.clone(),
            tombstones: tombstones.clone(),
            tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        },
    );
    Harness {
        _dir: dir,
        gateway,
        facts,
        vector,
        lexical,
        edges,
        watermarks,
        tombstones,
        judge,
    }
}

/// A judge script for `count` candidates, each with its neighbour count from
/// `neighbor_counts`. Order mirrors `judge_write_candidates`: one classify
/// batch, then per candidate its compares followed by its extract.
fn script_ok(
    count: usize,
    neighbor_counts: &[usize],
    relation: CompareRelation,
) -> Vec<ScriptedReply> {
    let item_ids: Vec<String> = (0..count).map(|i| format!("c{i}")).collect();
    let item_refs: Vec<&str> = item_ids.iter().map(String::as_str).collect();
    let mut script = vec![JudgeMock::classify_all(
        &item_refs,
        "semantic",
        Verdict::Yes,
        0.95,
    )];
    for &neighbors in neighbor_counts {
        for _ in 0..neighbors {
            script.push(ScriptedReply {
                op: mnemo2u::llm::judge_mock::JudgeOp::Compare,
                reply: JudgeReply::Compare(CompareResponse {
                    relation,
                    confidence: 0.9,
                    aspects: vec![],
                }),
            });
        }
        script.push(ScriptedReply {
            op: mnemo2u::llm::judge_mock::JudgeOp::Extract,
            reply: JudgeReply::Extract(ExtractResponse { fields: vec![] }),
        });
    }
    script
}

/// The deterministic fact id the gateway derives for a turn.
fn fact_id(conversation_id: &str, turn_id: &str) -> FactId {
    FactId(mnemo2u::core::text::compute_mdhash_id(
        &format!("{conversation_id}\u{1}{turn_id}"),
        "fact-",
    ))
}

#[tokio::test]
async fn commits_a_turn_into_every_store() {
    let harness = harness(
        script_ok(1, &[0], CompareRelation::DifferentFacts),
        gate_open(),
    )
    .await;
    let key = scope();
    let turns = vec![turn(
        "t1",
        "the deploy key rotates every ninety days and the rotated value is stored in the vault secret manager",
        1_000,
    )];

    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.written, 1);
    assert_eq!(outcome.considered, 1);

    // Truth row: kind from classify, verified because it settled, clean text.
    let id = fact_id("conv-1", "t1");
    let fact = harness
        .facts
        .get(&key, &id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(fact.kind, "semantic");
    assert_eq!(fact.trust_state, mnemo2u::core::types::TrustState::Verified);
    assert_eq!(fact.text, turns[0].turn.content);
    assert_eq!(fact.source_refs.len(), 1);
    assert_eq!(fact.source_refs[0].event_id, "t1");
    assert_eq!(fact.created_at, 1_000);

    // Derived indexes: the vector and the lexical search copy both landed.
    // The query vector is the turn's own embedding, which the mock embedder
    // derives from the text tokens.
    let embedder = mnemo2u::llm::mock::MockEmbedder::new();
    let query_vector = embedder
        .embed(&[turns[0].turn.content.clone()])
        .await
        .expect("embed")
        .remove(0);
    let vector_hits = harness
        .vector
        .search(&key, &query_vector, 10)
        .await
        .expect("search");
    assert!(vector_hits.iter().any(|(hit, _)| hit == &id));

    let lexical_hits = harness
        .lexical
        .search(&key, "vault", 10)
        .await
        .expect("search");
    assert!(
        lexical_hits.iter().any(|(hit, _)| hit == &id),
        "lexical search copy should carry the fact"
    );

    // Watermark advanced to the batch's last turn.
    let watermark = harness
        .watermarks
        .get_by_id("conv-1")
        .await
        .expect("watermark")
        .expect("row");
    assert_eq!(watermark["last_turn_id"], "t1");
    assert_eq!(watermark["last_timestamp"], 1_000);
}

#[tokio::test]
async fn replay_is_a_no_op_and_the_watermark_filters() {
    let harness = harness(
        script_ok(1, &[0], CompareRelation::DifferentFacts),
        gate_open(),
    )
    .await;
    let turns = vec![turn(
        "t1",
        "the vault holds the rotating deploy key material that the deployment pipeline reads",
        1_000,
    )];

    let first = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("first");
    assert_eq!(first.written, 1);

    // The same batch again: the watermark covers it, nothing re-judged.
    let second = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("second");
    assert_eq!(second.written, 0);
    assert_eq!(second.considered, 0);
    assert_eq!(harness.judge.calls(), 2, "replay must not call the judge");
}

#[tokio::test]
async fn superseded_facts_are_not_resurrected_by_replay() {
    let harness = harness(
        script_ok(1, &[1], CompareRelation::Contradicts),
        gate_open(),
    )
    .await;
    let key = scope();
    // A pre-existing fact the turn will contradict.
    let old = mnemo2u::core::types::Fact {
        id: FactId("fact-old".into()),
        scope: key.clone(),
        kind: "semantic".into(),
        // Near-identical to the turn below, so the mock embedder's cosine
        // clears the 0.85 compare floor and the judge gets a neighbour.
        text: "the old deploy key never rotates — instead it is rotated on a schedule".into(),
        source_refs: vec![],
        trust_state: mnemo2u::core::types::TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: 500,
        superseded_by: None,
        revision: 0,
    };
    harness.facts.put(&old).await.expect("put old");
    // Its vector, so the turn finds it as a neighbour.
    let embedding = mnemo2u::llm::mock::MockEmbedder::new()
        .embed(std::slice::from_ref(&old.text))
        .await
        .expect("embed")
        .remove(0);
    harness
        .vector
        .upsert(&key, &old.id, &embedding)
        .await
        .expect("vector");

    let turns = vec![turn(
        "t1",
        "the old deploy key never rotates — instead it is rotated on a schedule by the secret manager",
        1_000,
    )];
    // The seed's whole job is to be a neighbour, so pin that it actually is
    // one: without this, a cosine that drifts under the compare floor fails
    // later as an opaque judge-script mismatch instead of a clear cause.
    let probe = mnemo2u::llm::mock::MockEmbedder::new()
        .embed(std::slice::from_ref(&turns[0].turn.content))
        .await
        .expect("embed turn")
        .remove(0);
    let neighbours = harness
        .vector
        .search(&key, &probe, 5)
        .await
        .expect("vector search");
    let (nearest, similarity) = neighbours
        .into_iter()
        .find(|(id, _)| id == &FactId("fact-old".into()))
        .expect("the seeded fact is a neighbour of the turn");
    assert!(
        // The gateway's default `compare_min_cosine` (gateway.rs).
        similarity >= 0.85,
        "cosine {similarity} clears the compare floor, got {nearest:?}"
    );
    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.corrections, 1, "the contradiction should supersede");
    assert_eq!(outcome.written, 1);

    let superseded = harness
        .facts
        .get(&key, &FactId("fact-old".into()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        superseded.superseded_by,
        Some(fact_id("conv-1", "t1")),
        "the old row should carry the chain"
    );

    // The correction also landed in the graph, so the graph arm's ×1.5
    // multiplier for Corrects/Supersedes has an edge to weight.
    let supersede_edges = harness
        .edges
        .edges_by_rel(&key, mnemo2u::core::types::RelKind::Supersedes)
        .await
        .expect("edges");
    assert!(
        supersede_edges.iter().any(
            |edge| edge.src == fact_id("conv-1", "t1") && edge.dst == FactId("fact-old".into())
        ),
        "edges: {supersede_edges:?}"
    );

    // Replay after the correction: the fact exists, so it is skipped rather
    // than re-written — a re-write would clear `superseded_by` on the chain
    // target and resurrect the correction.
    let replay = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("replay");
    assert_eq!(replay.written, 0);
    let still_superseded = harness
        .facts
        .get(&key, &FactId("fact-old".into()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        still_superseded.superseded_by,
        Some(fact_id("conv-1", "t1")),
        "replay must not undo the correction"
    );
}

#[tokio::test]
async fn low_value_turns_cost_zero_judge_calls() {
    // An empty script: any judge call would panic on exhaustion, so a pass
    // proves the gate ran first. The production defaults apply here — both
    // turns are below the 24-token gate.
    let mut options = gate_open();
    options.cost.min_chunk_tokens = 24;
    let harness = harness(Vec::new(), options).await;
    let turns = vec![
        turn("t1", "ok", 1_000),
        turn("t2", "the key rotates", 1_001),
    ];

    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(
        harness.judge.calls(),
        0,
        "no judge call for low-value turns"
    );
    assert_eq!(outcome.skipped_low_value, 2);
    assert_eq!(outcome.written, 0);
}

#[tokio::test]
async fn zero_llm_capture_writes_without_a_judge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_string_lossy().to_string();
    let facts = Arc::new(
        TursoFactStore::open(&format!("{path}/facts.db"))
            .await
            .expect("facts"),
    );
    let watermarks = Arc::new(MemoryKv::new());
    let gateway = FactWriteGateway::new(
        gate_open(),
        mnemo2u::index::gateway::GatewayParts {
            facts: facts.clone(),
            vector: None,
            lexical: None,
            edges: None,
            embedder: Arc::new(mnemo2u::llm::mock::MockEmbedder::new()),
            judge: None,
            watermarks,
            tombstones: Arc::new(MemoryKv::new()),
            tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        },
    );

    let turns = vec![turn(
        "t1",
        "the harness replays the watermark range on every restart of the session",
        1_000,
    )];
    let outcome = gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.written, 1);

    let key = scope();
    let fact = facts
        .get(&key, &fact_id("conv-1", "t1"))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(fact.kind, "episodic", "zero-LLM default kind");
    assert_eq!(
        fact.trust_state,
        mnemo2u::core::types::TrustState::Candidate,
        "no judge means nothing is verified"
    );
}

#[tokio::test]
async fn judge_failure_aborts_and_leaves_the_watermark_put() {
    let harness = harness(
        vec![ScriptedReply {
            op: mnemo2u::llm::judge_mock::JudgeOp::Classify,
            reply: JudgeReply::Fail(JudgeError::Backend(
                mnemo2u::llm::envelope::ProviderError::Unavailable {
                    op: mnemo2u::llm::envelope::Op::Judge,
                },
            )),
        }],
        gate_open(),
    )
    .await;
    let key = scope();
    let turns = vec![turn(
        "t1",
        "the vault holds the rotating deploy key material that the deployment pipeline reads on every release",
        1_000,
    )];

    let error = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect_err("judge failure must abort");
    assert!(
        matches!(error, mnemo2u::index::gateway::GatewayError::Judge(_)),
        "{error}"
    );

    // Nothing written, and the watermark never advanced.
    assert!(
        harness
            .facts
            .get(&key, &fact_id("conv-1", "t1"))
            .await
            .expect("get")
            .is_none(),
        "a failed batch must not write rows"
    );
    assert!(
        harness
            .watermarks
            .get_by_id("conv-1")
            .await
            .expect("watermark")
            .is_none(),
        "the watermark must stay behind"
    );
}

#[tokio::test]
async fn same_fact_duplicates_are_not_written() {
    // The turn duplicates an existing fact: compare says SameFact, so the
    // gateway must skip the write rather than store a copy.
    let harness = harness(script_ok(1, &[1], CompareRelation::SameFact), gate_open()).await;
    let key = scope();
    let existing = mnemo2u::core::types::Fact {
        id: FactId("fact-old".into()),
        scope: key.clone(),
        kind: "semantic".into(),
        text: "the vault holds the rotating deploy key material that the deployment pipeline reads"
            .into(),
        source_refs: vec![],
        trust_state: mnemo2u::core::types::TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: 500,
        superseded_by: None,
        revision: 0,
    };
    harness.facts.put(&existing).await.expect("put");
    let embedding = mnemo2u::llm::mock::MockEmbedder::new()
        .embed(std::slice::from_ref(&existing.text))
        .await
        .expect("embed")
        .remove(0);
    harness
        .vector
        .upsert(&key, &existing.id, &embedding)
        .await
        .expect("vector");

    let turns = vec![turn(
        "t1",
        "the vault holds the rotating deploy key material that the deployment pipeline reads",
        1_000,
    )];
    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.deduplicated, 1);
    assert_eq!(outcome.written, 0);

    // R5 contract v2: the duplicate corroborates the existing row instead
    // of writing a copy — the new turn's source ref lands on the old row.
    let corroborated = harness
        .facts
        .get(&key, &existing.id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(corroborated.source_refs.len(), 1);
    assert_eq!(corroborated.source_refs[0].event_id, "t1");
    assert_eq!(
        corroborated.revision, 0,
        "corroboration is not a correction"
    );

    assert!(harness
        .facts
        .get(&key, &fact_id("conv-1", "t1"))
        .await
        .expect("get")
        .is_none());
}

/// Seed an existing candidate row plus its vector row, so the next turn's
/// neighbours include it and `compare` can say `SameFact`.
async fn seed_candidate(
    harness: &Harness,
    key: &ScopeKey,
    text: &str,
) -> mnemo2u::core::types::Fact {
    let existing = mnemo2u::core::types::Fact {
        id: FactId("fact-old".into()),
        scope: key.clone(),
        kind: "semantic".into(),
        text: text.into(),
        source_refs: vec![],
        trust_state: mnemo2u::core::types::TrustState::Candidate,
        valid_from: None,
        valid_to: None,
        created_at: 500,
        superseded_by: None,
        revision: 0,
    };
    harness.facts.put(&existing).await.expect("put");
    let embedding = mnemo2u::llm::mock::MockEmbedder::new()
        .embed(std::slice::from_ref(&existing.text))
        .await
        .expect("embed")
        .remove(0);
    harness
        .vector
        .upsert(key, &existing.id, &embedding)
        .await
        .expect("vector");
    existing
}

/// R5.AC2 (first half): restatements from one session append one ref per
/// distinct event but never add an independent source — the row stays a
/// candidate, and every restatement stays deduplicated.
#[tokio::test]
async fn same_session_restatements_do_not_add_independent_sources() {
    let harness = harness(
        script_ok(3, &[1, 1, 1], CompareRelation::SameFact),
        gate_open(),
    )
    .await;
    let key = scope();
    let text =
        "the vault holds the rotating deploy key material that the deployment pipeline reads";
    let existing = seed_candidate(&harness, &key, text).await;

    let turns = vec![
        turn("t1", text, 1_000),
        turn("t2", text, 2_000),
        turn("t3", text, 3_000),
    ];
    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.deduplicated, 3);
    assert_eq!(outcome.written, 0);

    let row = harness
        .facts
        .get(&key, &existing.id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.source_refs.len(), 3, "one ref per distinct event");
    let sessions: std::collections::BTreeSet<&str> = row
        .source_refs
        .iter()
        .map(|reference| reference.session_id.as_str())
        .collect();
    assert_eq!(
        sessions.len(),
        1,
        "one session is repetition, not corroboration"
    );
    assert_eq!(
        row.trust_state,
        mnemo2u::core::types::TrustState::Candidate,
        "a single session never promotes"
    );
    assert_eq!(row.revision, 0, "corroboration is not a correction");
}

/// R5.AC2 (second half): a second, independent session promotes the
/// candidate to verified, and the promotion survives a replay of the first
/// conversation (the watermark skips it, nothing is double-counted).
#[tokio::test]
async fn independent_conversations_promote_corroboration_to_verified() {
    let mut script = script_ok(1, &[1], CompareRelation::SameFact);
    script.extend(script_ok(1, &[1], CompareRelation::SameFact));
    let harness = harness(script, gate_open()).await;
    let key = scope();
    let text =
        "the vault holds the rotating deploy key material that the deployment pipeline reads";
    let existing = seed_candidate(&harness, &key, text).await;

    let first = harness
        .gateway
        .commit_turns("conv-1", &[turn("t1", text, 1_000)])
        .await
        .expect("commit 1");
    assert_eq!(first.deduplicated, 1);

    let second = harness
        .gateway
        .commit_turns("conv-2", &[turn_in("conv-2", "t9", text, 2_000)])
        .await
        .expect("commit 2");
    assert_eq!(second.deduplicated, 1);

    let row = harness
        .facts
        .get(&key, &existing.id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.source_refs.len(), 2);
    let sessions: std::collections::BTreeSet<&str> = row
        .source_refs
        .iter()
        .map(|reference| reference.session_id.as_str())
        .collect();
    assert_eq!(sessions.len(), 2);
    assert_eq!(
        row.trust_state,
        mnemo2u::core::types::TrustState::Verified,
        "two independent sessions promote the row"
    );

    let replay = harness
        .gateway
        .commit_turns("conv-1", &[turn("t1", text, 1_000)])
        .await
        .expect("replay");
    assert_eq!(replay.written, 0, "the watermark covers the replayed turn");
    let after = harness
        .facts
        .get(&key, &existing.id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(after.source_refs.len(), 2);
    assert_eq!(
        after.trust_state,
        mnemo2u::core::types::TrustState::Verified
    );
}

/// R5.AC3: a value the host rejected never re-enters memory — the screen
/// runs before any judge call, so 100 re-extraction attempts (across
/// casing/whitespace variants, proving the canonical key) burn zero model
/// attention and write zero rows; the rejected row itself leaves recall
/// while keeping its history.
#[tokio::test]
async fn rejected_values_never_reenter_and_leave_recall() {
    let harness = harness(
        script_ok(1, &[0], CompareRelation::DifferentFacts),
        gate_open(),
    )
    .await;
    let key = scope();
    let text =
        "the vault holds the rotating deploy key material that the deployment pipeline reads";

    let first = harness
        .gateway
        .commit_turns("conv-1", &[turn("t1", text, 1_000)])
        .await
        .expect("commit");
    assert_eq!(first.written, 1);
    let rejected_id = fact_id("conv-1", "t1");

    harness
        .gateway
        .reject(&rejected_id, "the host explicitly rejected this value")
        .await
        .expect("reject");
    let row = harness
        .facts
        .get(&key, &rejected_id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.trust_state, mnemo2u::core::types::TrustState::Rejected);
    assert!(!row.is_current(), "a rejected row leaves the current view");
    assert_eq!(
        harness
            .facts
            .search(&key, "vault rotating deploy key material", 10)
            .await
            .expect("search")
            .len(),
        0,
        "a rejected row must not surface in recall"
    );
    assert_eq!(
        harness.tombstones.all_keys().await.expect("keys").len(),
        1,
        "the rejection is a durable ledger entry"
    );

    // 100 re-extraction attempts: the same value in casing and whitespace
    // variants — every one canonicalises onto the same tombstone.
    let calls_before = harness.judge.calls();
    let attempts: Vec<SessionTurn> = (0..100)
        .map(|index| {
            let variant = match index % 3 {
                0 => text.to_string(),
                1 => text.to_uppercase(),
                _ => text.replace(' ', "  "),
            };
            turn_in("conv-2", &format!("r{index}"), &variant, 2_000 + index)
        })
        .collect();
    let outcome = harness
        .gateway
        .commit_turns("conv-2", &attempts)
        .await
        .expect("commit attempts");
    assert_eq!(outcome.tombstoned, 100, "every attempt hits the ledger");
    assert_eq!(outcome.written, 0, "zero resurrection");
    assert_eq!(
        harness.judge.calls(),
        calls_before,
        "the screen runs before any judge call"
    );
}

/// R5.AC4: deleting a source event invalidates exactly the facts that cite
/// it — nothing else moves, the invalidated row leaves recall with its
/// window closed, and a replay/re-drive of the turn does not resurrect it.
#[tokio::test]
async fn invalidate_source_closes_citing_facts_and_stays_closed() {
    let harness = harness(
        script_ok(2, &[0, 0], CompareRelation::DifferentFacts),
        gate_open(),
    )
    .await;
    let key = scope();
    let cited =
        "the vault holds the rotating deploy key material that the deployment pipeline reads";
    let untouched = "the harness replays the watermark range on every restart of the session";

    let turns = vec![turn("t1", cited, 1_000), turn("t2", untouched, 2_000)];
    let first = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(first.written, 2);

    let invalidated = harness
        .gateway
        .invalidate_source("conv-1", "t1", 9_000)
        .await
        .expect("invalidate");
    assert_eq!(invalidated, 1, "only the citing fact is touched");

    let closed = harness
        .facts
        .get(&key, &fact_id("conv-1", "t1"))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(closed.valid_to, Some(9_000));
    assert_eq!(closed.revision, 1);
    assert!(!closed.is_current());

    let survivor = harness
        .facts
        .get(&key, &fact_id("conv-1", "t2"))
        .await
        .expect("get")
        .expect("row");
    assert!(survivor.is_current(), "unrelated rows keep their window");

    let hits = harness
        .facts
        .search(&key, "deploy key material vault", 10)
        .await
        .expect("search");
    assert!(
        hits.iter().all(|(id, _)| id != &fact_id("conv-1", "t1")),
        "an invalidated row must leave recall"
    );

    // A re-drive (simulating a crash between the row write and the
    // watermark) hits the row guard: the existing, window-closed row is
    // skipped rather than rewritten.
    harness
        .watermarks
        .remove(&["conv-1".to_string()])
        .await
        .expect("clear watermark");
    let redrive = harness
        .gateway
        .commit_turns("conv-1", &[turn("t1", cited, 1_000)])
        .await
        .expect("redrive");
    assert_eq!(redrive.written, 0);
    assert_eq!(redrive.skipped_known, 1, "{redrive:?}");
    let still = harness
        .facts
        .get(&key, &fact_id("conv-1", "t1"))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        still.valid_to,
        Some(9_000),
        "re-drive must not reopen the window"
    );
}

#[tokio::test]
async fn extracted_anchors_land_in_the_search_copy_not_the_text() {
    let harness = harness(
        vec![
            JudgeMock::classify_all(&["c0"], "semantic", Verdict::Yes, 0.95),
            ScriptedReply {
                op: mnemo2u::llm::judge_mock::JudgeOp::Extract,
                reply: JudgeReply::Extract(ExtractResponse {
                    fields: vec![ExtractResult {
                        field_id: "error_code".into(),
                        value: Some("ERR-1234".into()),
                        status: ExtractStatus::Auto,
                    }],
                }),
            },
        ],
        gate_open(),
    )
    .await;
    let key = scope();
    let turns = vec![turn(
        "t1",
        "the deployment pipeline failed with ERR-1234 while rotating the vault deploy key material",
        1_000,
    )];

    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.written, 1);

    let id = fact_id("conv-1", "t1");
    let fact = harness
        .facts
        .get(&key, &id)
        .await
        .expect("get")
        .expect("row");
    // The truth row keeps the turn text verbatim: the extracted anchor is
    // appended only to the lexical search copy, never to the stored body.
    assert_eq!(fact.text, turns[0].turn.content);

    // The lexical arm finds it by the extracted anchor.
    let hits = harness
        .lexical
        .search(&key, "ERR-1234", 10)
        .await
        .expect("search");
    assert!(hits.iter().any(|(hit, _)| hit == &id));
}

#[tokio::test]
async fn review_verdicts_keep_the_fact_unverified() {
    let harness = harness(
        vec![
            JudgeMock::classify_all(&["c0"], "semantic", Verdict::Review, 0.4),
            ScriptedReply {
                op: mnemo2u::llm::judge_mock::JudgeOp::Extract,
                reply: JudgeReply::Extract(ExtractResponse { fields: vec![] }),
            },
        ],
        gate_open(),
    )
    .await;
    let key = scope();
    let turns = vec![turn(
        "t1",
        "the vault holds the rotating deploy key material that the deployment pipeline reads that the deployment pipeline reads on every release",
        1_000,
    )];

    let outcome = harness
        .gateway
        .commit_turns("conv-1", &turns)
        .await
        .expect("commit");
    assert_eq!(outcome.written, 1);
    let fact = harness
        .facts
        .get(&key, &fact_id("conv-1", "t1"))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        fact.trust_state,
        mnemo2u::core::types::TrustState::Candidate,
        "an unresolved classification must not promote the row"
    );
    assert_eq!(fact.kind, "episodic", "no class id on a Review");
}
