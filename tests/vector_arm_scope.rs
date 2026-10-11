//! LanceDB fact-vector arm contract (stage doc 03 T2): in-query scope
//! pre-filtering, cosine-similarity ranking with a deterministic tie-break,
//! upsert/overwrite + remove lifecycle, and the embedding cache key.
//!
//! Fixture: `dim = 8` hand-built vectors over two workspaces. `ws-b` carries
//! the vector most aligned with the query (`b-hot`, cosine 1.0) so the
//! cross-workspace negation is not vacuous — a pre-filtered `ws-a` search must
//! still never surface it.

use std::collections::BTreeSet;

use mnemo2u::core::traits::FactVectorStore;
use mnemo2u::core::types::{FactId, ScopeKey};
use mnemo2u::store::lance_facts::{fact_embedding_cache_key, LanceFactVector};

const DIM: usize = 8;

/// The query every search in this test issues.
fn query() -> [f32; DIM] {
    [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

fn scope(ws: &str, sess: Option<&str>, task: Option<&str>) -> ScopeKey {
    ScopeKey {
        workspace_id: ws.into(),
        session_id: sess.map(str::to_string),
        task_id: task.map(str::to_string),
    }
}

/// One stored fact: the writer's storage scope and its 8-dim embedding.
struct Row {
    id: &'static str,
    scope: ScopeKey,
    vec: [f32; DIM],
}

/// The shared fixture. Fact ids are globally unique, so within any single
/// scope a search result never carries one twice.
fn fixture() -> Vec<Row> {
    vec![
        // ws-a / s1 — one vector at cosine 1/√2 and two at cosine 0 (an exact
        // tie between a-alpha and a-zeta).
        Row {
            id: "a-mid",
            scope: scope("ws-a", Some("s1"), None),
            vec: [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        Row {
            id: "a-alpha",
            scope: scope("ws-a", Some("s1"), None),
            vec: [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        Row {
            id: "a-zeta",
            scope: scope("ws-a", Some("s1"), None),
            vec: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        // ws-a / s2 — the top ws-a fact (cosine 2/√5 ≈ 0.894), and a task-owned
        // fact reusing `a-mid`'s vector so it ties with `a-mid` exactly.
        Row {
            id: "a-top",
            scope: scope("ws-a", Some("s2"), None),
            vec: [2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        Row {
            id: "a-task",
            scope: scope("ws-a", Some("s2"), Some("t1")),
            vec: [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        // ws-b — unconstrained. `b-hot` is the most query-aligned vector in the
        // whole fixture (cosine 1.0), beating every ws-a fact.
        Row {
            id: "b-hot",
            scope: scope("ws-b", None, None),
            vec: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        Row {
            id: "b-mid",
            scope: scope("ws-b", None, None),
            vec: [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
    ]
}

/// The ids a `scope` is entitled to see under [`ScopeKey::admits`], as a set
/// (order-independent) — the ground truth every narrowing assertion checks.
fn id_set(rows: &[Row], scope: &ScopeKey) -> BTreeSet<String> {
    rows.iter()
        .filter(|row| scope.admits(&row.scope))
        .map(|row| row.id.to_string())
        .collect()
}

async fn upsert_all(store: &LanceFactVector, rows: &[Row]) {
    for row in rows {
        store
            .upsert(&row.scope, &FactId(row.id.to_string()), &row.vec)
            .await
            .expect("upsert");
    }
}

/// The ids in a search result, in order — used for exact-ordering checks.
fn ids(hits: &[(FactId, f64)]) -> Vec<String> {
    hits.iter().map(|(id, _)| id.0.clone()).collect()
}

/// A search result must never carry the same fact id twice.
fn assert_distinct(hits: &[(FactId, f64)]) {
    let unique: BTreeSet<&str> = hits.iter().map(|(id, _)| id.0.as_str()).collect();
    assert_eq!(
        unique.len(),
        hits.len(),
        "duplicate fact ids in one search result: {unique:?}"
    );
}

/// Cosine similarity, mirroring the arm's scoring口径 (for hand checks only).
fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum();
    let na: f64 = a
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    let nb: f64 = b
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    dot / (na * nb)
}

#[tokio::test]
async fn vector_arm_scope_prefilter_ranking_lifecycle_and_cache_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().expect("path");

    let store = LanceFactVector::new(path, DIM).await.expect("open");
    let rows = fixture();
    upsert_all(&store, &rows).await;
    let q = query();

    // --- 1. Cross-workspace negation (proves the pre-filter, not a post-filter). ---
    // b-hot (cosine 1.0) beats every ws-a fact, so an unscoped search would rank
    // it #1; a ws-a search must still never leak a ws-b id.
    let ws_a = scope("ws-a", None, None);
    let a_hits = store.search(&ws_a, &q, 20).await.expect("search ws-a");
    assert_distinct(&a_hits);
    let b_ids = ["b-hot".to_string(), "b-mid".to_string()];
    for id in ids(&a_hits).iter() {
        assert!(
            !b_ids.contains(id),
            "ws-a search leaked a ws-b id {id:?}: {a_hits:?}"
        );
    }
    // The ws-a search only ever returns ws-a's own facts.
    let got: BTreeSet<String> = BTreeSet::from_iter(ids(&a_hits).into_iter());
    assert_eq!(
        got,
        id_set(&rows, &ws_a),
        "ws-a scope must see exactly its own facts"
    );

    // The ws-b scope sees b's own facts, with the most-aligned one at the top.
    let ws_b = scope("ws-b", None, None);
    let b_hits = store.search(&ws_b, &q, 20).await.expect("search ws-b");
    assert_distinct(&b_hits);
    assert_eq!(
        ids(&b_hits),
        vec!["b-hot".to_string(), "b-mid".to_string()],
        "ws-b scope must recall its own facts, most similar first: {b_hits:?}"
    );

    // --- 2. Session / task narrowing matches ScopeKey::admits. ---
    let s1 = scope("ws-a", Some("s1"), None);
    let s1_hits = store.search(&s1, &q, 20).await.expect("search s1");
    assert_distinct(&s1_hits);
    assert_eq!(
        BTreeSet::from_iter(ids(&s1_hits).into_iter()),
        id_set(&rows, &s1),
        "s1 scope: {s1_hits:?}"
    );

    // An unconstrained session sees the whole workspace; some("s1") sees only s1.
    assert!(
        ids(&s1_hits).iter().all(|id| id.starts_with("a-")),
        "s1 scope must not see s2 or b: {s1_hits:?}"
    );

    let s2 = scope("ws-a", Some("s2"), None);
    let s2_hits = store.search(&s2, &q, 20).await.expect("search s2");
    assert_eq!(
        BTreeSet::from_iter(ids(&s2_hits).into_iter()),
        id_set(&rows, &s2),
        "s2 scope: {s2_hits:?}"
    );

    // Task narrowing: t1 keeps only the task-owned row; an empty task scope sees
    // everything under that session.
    let s2_t1 = scope("ws-a", Some("s2"), Some("t1"));
    let s2_t1_hits = store.search(&s2_t1, &q, 20).await.expect("search s2 t1");
    assert_eq!(
        ids(&s2_t1_hits),
        vec!["a-task".to_string()],
        "s2 + t1 must narrow to the task-owned row: {s2_t1_hits:?}"
    );
    let s1_t1 = scope("ws-a", Some("s1"), Some("t1"));
    let s1_t1_hits = store.search(&s1_t1, &q, 20).await.expect("search s1 t1");
    assert!(
        s1_t1_hits.is_empty(),
        "no s1 row carries task t1: {s1_t1_hits:?}"
    );

    // --- 3. Ranking: similarity descending, lexicographic fact_id tie-break. ---
    // s1: a-mid (cosine 1/√2) strictly beats the two cosine-0 vectors, which tie
    // and therefore fall back to fact_id ascending (a-alpha < a-zeta).
    assert_eq!(
        ids(&s1_hits),
        vec![
            "a-mid".to_string(),
            "a-alpha".to_string(),
            "a-zeta".to_string()
        ],
        "s1 ordering (similarity desc, tie by id asc): {s1_hits:?}"
    );
    // Full ws-a: a-top first; a-mid == a-task (identical vectors → identical
    // distance) tie-breaks by id; a-alpha == a-zeta likewise.
    assert_eq!(
        ids(&a_hits),
        vec![
            "a-top".to_string(),
            "a-mid".to_string(),
            "a-task".to_string(),
            "a-alpha".to_string(),
            "a-zeta".to_string(),
        ],
        "ws-a ordering (similarity desc, tie by id asc): {a_hits:?}"
    );
    // hand-check the top score against the cosine口径.
    let top = &a_hits[0];
    let want_top = cosine(&q, &[2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert!(
        (top.1 - want_top).abs() < 1e-6,
        "top similarity must match cosine: got {} want {}",
        top.1,
        want_top
    );

    // limit truncates the same query.
    let two = store.search(&ws_a, &q, 2).await.expect("limit 2");
    assert_eq!(
        ids(&two),
        vec!["a-top".to_string(), "a-mid".to_string()],
        "limit must truncate: {two:?}"
    );

    // --- 4a. Upsert overwrites the same (scope, id): new vector wins, no dupes. ---
    // a-mid was at cosine 1/√2; re-embed it under its own scope as the unit
    // vector so it now sits at cosine 1.0 and re-ranks to the s1 top.
    store
        .upsert(
            &scope("ws-a", Some("s1"), None),
            &FactId("a-mid".into()),
            &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .await
        .expect("upsert overwrite");
    let s1_re = store
        .search(&s1, &q, 20)
        .await
        .expect("search after upsert");
    assert_distinct(&s1_re);
    assert_eq!(
        ids(&s1_re),
        vec![
            "a-mid".to_string(),
            "a-alpha".to_string(),
            "a-zeta".to_string()
        ],
        "a-mid must re-rank to the top with its new vector: {s1_re:?}"
    );
    assert!(
        (s1_re[0].1 - 1.0).abs() < 1e-6,
        "the over-written vector must now score ~1.0: {}",
        s1_re[0].1
    );

    // --- 4b. Remove drops the id; removing an absent combo is not an error. ---
    store
        .remove(&s1, &FactId("a-alpha".into()))
        .await
        .expect("remove a-alpha");
    let s1_removed = store
        .search(&s1, &q, 20)
        .await
        .expect("search after remove");
    assert!(
        ids(&s1_removed).iter().all(|id| id != "a-alpha"),
        "a-alpha must be gone after remove: {s1_removed:?}"
    );
    assert_eq!(
        ids(&s1_removed),
        vec!["a-mid".to_string(), "a-zeta".to_string()],
        "remove must drop only the targeted id: {s1_removed:?}"
    );
    // A combo that was never stored removes zero rows and still succeeds.
    store
        .remove(&s1, &FactId("a-ghost".into()))
        .await
        .expect("remove absent combo is Ok");

    // --- 5. Embedding cache key: stable, component-sensitive, None≡Some(""). ---
    let base = scope("ws-a", Some("s1"), Some("t1"));
    let model = "bge-small";
    let text = "the quick brown fox";
    let k_base = fact_embedding_cache_key(&base, model, text);
    assert_eq!(
        k_base,
        fact_embedding_cache_key(&base, model, text),
        "cache key must be stable for identical input"
    );

    // any single component change flips the key.
    assert_ne!(
        fact_embedding_cache_key(&scope("ws-b", Some("s1"), Some("t1")), model, text),
        k_base,
        "workspace component must matter"
    );
    assert_ne!(
        fact_embedding_cache_key(&base, model, "the quick brown foxes"),
        k_base,
        "text component must matter"
    );
    assert_ne!(
        fact_embedding_cache_key(&base, "bge-m3", text),
        k_base,
        "model component must matter"
    );
    assert_ne!(
        fact_embedding_cache_key(&scope("ws-a", Some("s2"), Some("t1")), model, text),
        k_base,
        "session component must matter"
    );
    assert_ne!(
        fact_embedding_cache_key(&scope("ws-a", Some("s1"), Some("t2")), model, text),
        k_base,
        "task component must matter"
    );

    // The one exception: an unconstrained component (None) and the empty-string
    // form of the same component are indistinguishable after normalization.
    assert_eq!(
        fact_embedding_cache_key(&scope("ws-a", None, Some("t1")), model, text),
        fact_embedding_cache_key(&scope("ws-a", Some(""), Some("t1")), model, text),
        "session None must equal session Some(\"\")"
    );
    assert_eq!(
        fact_embedding_cache_key(&scope("ws-a", Some("s1"), None), model, text),
        fact_embedding_cache_key(&scope("ws-a", Some("s1"), Some("")), model, text),
        "task None must equal task Some(\"\")"
    );

    // --- Reopening the directory keeps serving the vectors (the index is on disk). ---
    drop(store);
    let reopened = LanceFactVector::new(path, DIM).await.expect("reopen");
    let again = reopened
        .search(&s1, &q, 20)
        .await
        .expect("search after reopen");
    assert_eq!(
        ids(&again),
        vec!["a-mid".to_string(), "a-zeta".to_string()],
        "reopen lost the remove/overwrite state: {again:?}"
    );
}
