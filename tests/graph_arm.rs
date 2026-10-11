//! Turso fact-edge (graph) arm contract (stage doc 03 T4): undirected n-hop
//! traversal under a scope predicate, upsert/remove lifecycle, per-rel
//! filtering, and lossless field round-trip.
//!
//! One database file per test, several workspaces: workspace A holds a short
//! fact chain with a reverse edge and a cycle; workspace B mirrors a *longer*
//! chain on the same fact ids as interference — if the scope predicate ever
//! leaked, B's extra node `e` would surface in an A-only traversal.

use std::collections::BTreeSet;

use mnemo2u::core::traits::FactEdgeStore;
use mnemo2u::core::types::{Edge, FactId, RelKind, ScopeKey};
use mnemo2u::store::turso_edges::TursoFactEdges;

fn scope(ws: &str, sess: Option<&str>, task: Option<&str>) -> ScopeKey {
    ScopeKey {
        workspace_id: ws.to_string(),
        session_id: sess.map(str::to_string),
        task_id: task.map(str::to_string),
    }
}

fn edge(src: &str, dst: &str, rel: RelKind, weight: f64, ev: &str) -> Edge {
    Edge {
        src: FactId(src.to_string()),
        dst: FactId(dst.to_string()),
        rel,
        weight,
        source_event_id: ev.to_string(),
    }
}

async fn upsert(store: &TursoFactEdges, sc: &ScopeKey, edges: &[Edge]) {
    store
        .upsert_edges(sc, edges)
        .await
        .unwrap_or_else(|err| panic!("upsert under {sc:?}: {err:?}"));
}

/// Unique identity of an edge: (src, dst, rel).
fn ek(e: &Edge) -> (String, String, String) {
    (e.src.0.clone(), e.dst.0.clone(), format!("{:?}", e.rel))
}

fn ek_set(edges: &[Edge]) -> BTreeSet<(String, String, String)> {
    edges.iter().map(ek).collect()
}

/// Every fact id (source or target) that appears in `edges`.
fn fact_ids(edges: &[Edge]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for e in edges {
        ids.insert(e.src.0.clone());
        ids.insert(e.dst.0.clone());
    }
    ids
}

/// Every persisted source-event id that appears in `edges`.
fn event_ids(edges: &[Edge]) -> BTreeSet<String> {
    edges.iter().map(|e| e.source_event_id.clone()).collect()
}

/// Workspace A fixture: chain a→b→c→d, a reverse edge x→a, a cycle edge
/// d→a, and varying rel kinds and weights. All under the unconstrained A
/// scope.
fn fixture_a() -> Vec<Edge> {
    vec![
        edge("a", "b", RelKind::DependsOn, 0.25, "e1"),
        edge("b", "c", RelKind::DependsOn, 0.5, "e2"),
        edge("c", "d", RelKind::AboutTask, 0.75, "e3"),
        edge("x", "a", RelKind::Corrects, 1.0, "e4"),
        edge("d", "a", RelKind::Related, 0.1, "e5"), // cycle a→b→c→d→a
    ]
}

/// Workspace B fixture: a longer chain on the same fact ids plus B-only node
/// `e`, distinct source-event ids (`wb-*`) so any leak is detectable.
fn fixture_b() -> Vec<Edge> {
    vec![
        edge("a", "b", RelKind::DependsOn, 0.25, "wb-1"),
        edge("b", "c", RelKind::DependsOn, 0.5, "wb-2"),
        edge("c", "d", RelKind::AboutTask, 0.75, "wb-3"),
        edge("d", "e", RelKind::Related, 0.9, "wb-4"),
    ]
}

/// Assert `got` contains `want` and nothing else (set equality).
fn assert_edge_set(got: &[Edge], want: &[Edge], label: &str) {
    let got_set = ek_set(got);
    let want_set = ek_set(want);
    assert_eq!(
        got_set, want_set,
        "{label}: got {got_set:?}, want {want_set:?}"
    );
}

// --- 1. Hops boundaries, cycle termination, cross-scope negation. ---

#[tokio::test]
async fn graph_arm_hops_boundaries_and_cross_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("edges.db");
    let path = path.to_str().expect("path");

    let store = TursoFactEdges::new(path).await.expect("open");
    let a_root = scope("ws-a", None, None);
    let b_root = scope("ws-b", None, None);
    upsert(&store, &a_root, &fixture_a()).await;
    upsert(&store, &b_root, &fixture_b()).await;

    // Empty seeds or zero hops is a no-op neighborhood.
    assert!(store
        .neighbors(&[], 3, &a_root)
        .await
        .expect("empty seeds")
        .is_empty());
    assert!(store
        .neighbors(&[FactId("a".into())], 0, &a_root)
        .await
        .expect("hops 0")
        .is_empty());

    // hops = 1: the seed's direct undirected neighborhood. It sees the
    // forward edge a→b and the reverse-stored x→a (and the cycle edge d→a,
    // both adjacent to `a`), but no edge touches `c` yet.
    let h1 = store
        .neighbors(&[FactId("a".into())], 1, &a_root)
        .await
        .expect("hops 1");
    let h1_set = ek_set(&h1);
    assert!(
        h1_set.contains(&("a".into(), "b".into(), "DependsOn".into())),
        "h1 sees a→b: {h1_set:?}"
    );
    assert!(
        h1_set.contains(&("x".into(), "a".into(), "Corrects".into())),
        "h1 sees reverse x→a: {h1_set:?}"
    );
    assert!(
        !fact_ids(&h1).iter().any(|id| id == "c"),
        "h1 must not reach c: {h1_set:?}"
    );

    // hops = 2: `c` is now reached (via b→c and c→d).
    let h2 = store
        .neighbors(&[FactId("a".into())], 2, &a_root)
        .await
        .expect("hops 2");
    assert!(
        ek_set(&h2).contains(&("b".into(), "c".into(), "DependsOn".into())),
        "h2 reaches c: {h2:?}"
    );

    // Cycle graph: a→b→c→d→a. A deep traversal must terminate (finite
    // result) and report every edge at most once — no duplicate rows.
    let h5 = store
        .neighbors(&[FactId("a".into())], 5, &a_root)
        .await
        .expect("hops 5");
    let h5_keys = ek_set(&h5);
    assert_eq!(
        h5_keys.len(),
        h5.len(),
        "no duplicate edges in a cyclic graph: {h5:?}",
    );
    assert_edge_set(
        &h5,
        &[
            edge("a", "b", RelKind::DependsOn, 0.25, "e1"),
            edge("b", "c", RelKind::DependsOn, 0.5, "e2"),
            edge("c", "d", RelKind::AboutTask, 0.75, "e3"),
            edge("x", "a", RelKind::Corrects, 1.0, "e4"),
            edge("d", "a", RelKind::Related, 0.1, "e5"),
        ],
        "A-only neighborhood is exactly the five A edges",
    );

    // Cross-scope negation: B's longer chain (with B-only node `e`) must not
    // leak into an A traversal, even though the same fact ids exist in B and
    // B's chain is longer.
    let h5_events = event_ids(&h5);
    let leaked_events: Vec<_> = h5_events.iter().filter(|ev| ev.starts_with("wb")).collect();
    assert!(
        leaked_events.is_empty(),
        "no ws-b row leaked into ws-a: {leaked_events:?}"
    );
    assert!(
        !fact_ids(&h5).iter().any(|id| id == "e"),
        "ws-b-only node e must not appear in a ws-a traversal: {h5:?}",
    );
}

// --- 2. Scope narrowing vs relaxation follows `ScopeKey::admits`. ---

#[tokio::test]
async fn graph_arm_scope_narrowing_admits_semantics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("edges.db");
    let path = path.to_str().expect("path");

    let store = TursoFactEdges::new(path).await.expect("open");

    // Same src/dst written under two scopes so scope is the only difference.
    upsert(
        &store,
        &scope("ws-a", None, None),
        &[edge("a1", "b1", RelKind::DependsOn, 0.3, "n1")],
    )
    .await;
    upsert(
        &store,
        &scope("ws-a", Some("s1"), None),
        &[
            edge("a1", "b1", RelKind::AboutTask, 0.7, "s1"),
            edge("a3", "b3", RelKind::DependsOn, 0.1, "s1"),
        ],
    )
    .await;
    upsert(
        &store,
        &scope("ws-a", Some("s1"), Some("t1")),
        &[edge("a3", "b3", RelKind::Corrects, 0.9, "s1t1")],
    )
    .await;

    let seed = |id: &str| vec![FactId(id.into())];

    // Relaxation: an unconstrained session admits every session (and every
    // session-authored row) under the workspace — both a1→b1 rows surface.
    let open = store
        .neighbors(&seed("a1"), 1, &scope("ws-a", None, None))
        .await
        .expect("relax");
    assert_edge_set(
        &open,
        &[
            edge("a1", "b1", RelKind::DependsOn, 0.3, "n1"),
            edge("a1", "b1", RelKind::AboutTask, 0.7, "s1"),
        ],
        "session=None admits all session rows",
    );

    // Narrowing: session s1 admits only rows stored under s1 — the s1 row
    // (AboutTask), not the unconstrained one (DependsOn).
    let s1 = store
        .neighbors(&seed("a1"), 1, &scope("ws-a", Some("s1"), None))
        .await
        .expect("narrow s1");
    assert_edge_set(
        &s1,
        &[edge("a1", "b1", RelKind::AboutTask, 0.7, "s1")],
        "session=s1 admits only s1 rows",
    );

    // A session the rows were never written under admits nothing.
    let s2 = store
        .neighbors(&seed("a1"), 1, &scope("ws-a", Some("s2"), None))
        .await
        .expect("narrow s2");
    assert!(s2.is_empty(), "session=s2 admits nothing: {s2:?}");

    // Task narrowing composes with session: task=None (within s1) admits both
    // a3→b3 rows; task=t1 admits only the t1 row.
    let s1_open_task = store
        .neighbors(&seed("a3"), 1, &scope("ws-a", Some("s1"), None))
        .await
        .expect("s1 task=None");
    assert_edge_set(
        &s1_open_task,
        &[
            edge("a3", "b3", RelKind::DependsOn, 0.1, "s1"),
            edge("a3", "b3", RelKind::Corrects, 0.9, "s1t1"),
        ],
        "task=None within s1 admits all tasks",
    );

    let s1_t1 = store
        .neighbors(&seed("a3"), 1, &scope("ws-a", Some("s1"), Some("t1")))
        .await
        .expect("s1 t1");
    assert_edge_set(
        &s1_t1,
        &[edge("a3", "b3", RelKind::Corrects, 0.9, "s1t1")],
        "task=t1 admits only the t1 row",
    );

    let s1_t9 = store
        .neighbors(&seed("a3"), 1, &scope("ws-a", Some("s1"), Some("t9")))
        .await
        .expect("s1 t9");
    assert!(s1_t9.is_empty(), "task=t9 admits nothing: {s1_t9:?}");
}

// --- 3. Upsert override + remove lifecycle. ---

#[tokio::test]
async fn graph_arm_upsert_override_and_remove() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("edges.db");
    let path = path.to_str().expect("path");

    let store = TursoFactEdges::new(path).await.expect("open");
    let a_root = scope("ws-a", None, None);

    // Seed the main chain so `a` has both outgoing (a→b) and incoming (x→a)
    // edges.
    upsert(&store, &a_root, &fixture_a()).await;

    // Override: re-upsert the same primary key (scope, a, b, DependsOn) with
    // new weight + event id. INSERT OR REPLACE means one logical row, not two.
    upsert(
        &store,
        &a_root,
        &[edge("a", "b", RelKind::DependsOn, 0.8, "v2")],
    )
    .await;
    let depends = store
        .edges_by_rel(&a_root, RelKind::DependsOn)
        .await
        .expect("dep");
    let ab: Vec<_> = depends
        .iter()
        .filter(|e| e.src.0 == "a" && e.dst.0 == "b")
        .collect();
    assert_eq!(
        ab.len(),
        1,
        "same primary key collapses to one row after override: {ab:?}"
    );
    assert_eq!(ab[0].weight, 0.8, "override replaces the weight: {ab:?}");
    assert_eq!(
        ab[0].source_event_id, "v2",
        "override replaces the event: {ab:?}"
    );

    // Remove nonexistent source: no outgoing edges → Ok, not an error.
    store
        .remove_edges_from(&a_root, &FactId("zzz".into()))
        .await
        .expect("remove missing");

    // Remove `a`'s outgoing edges: a→b goes, the incoming x→a stays.
    store
        .remove_edges_from(&a_root, &FactId("a".into()))
        .await
        .expect("remove a");
    let h1 = store
        .neighbors(&[FactId("a".into())], 1, &a_root)
        .await
        .expect("post-remove");
    let h1_set = ek_set(&h1);
    assert!(
        !h1_set.contains(&("a".into(), "b".into(), "DependsOn".into())),
        "a's outgoing edge is gone after remove: {h1_set:?}"
    );
    assert!(
        h1_set.contains(&("x".into(), "a".into(), "Corrects".into())),
        "incoming x→a survives removing a's out-edges: {h1_set:?}"
    );
}

// --- 4. edges_by_rel filters by rel; different scopes do not mix. ---

#[tokio::test]
async fn graph_arm_edges_by_rel_isolated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("edges.db");
    let path = path.to_str().expect("path");

    let store = TursoFactEdges::new(path).await.expect("open");
    let a_root = scope("ws-a", None, None);
    let b_root = scope("ws-b", None, None);

    upsert(
        &store,
        &a_root,
        &[
            edge("m", "n", RelKind::Corrects, 0.5, "m1"),
            edge("n", "o", RelKind::Corrects, 0.6, "m2"),
            edge("p", "q", RelKind::AboutTask, 0.4, "m3"),
        ],
    )
    .await;
    // A different rel under a *different* scope, same fact ids as above.
    upsert(
        &store,
        &b_root,
        &[edge("m", "n", RelKind::Corrects, 0.9, "wb-mn")],
    )
    .await;

    // A: two Corrects edges, no AboutTask.
    let a_corrects = store
        .edges_by_rel(&a_root, RelKind::Corrects)
        .await
        .expect("a corrects");
    assert_edge_set(
        &a_corrects,
        &[
            edge("m", "n", RelKind::Corrects, 0.5, "m1"),
            edge("n", "o", RelKind::Corrects, 0.6, "m2"),
        ],
        "A's Corrects edges only",
    );
    let a_about = store
        .edges_by_rel(&a_root, RelKind::AboutTask)
        .await
        .expect("a about");
    assert_edge_set(
        &a_about,
        &[edge("p", "q", RelKind::AboutTask, 0.4, "m3")],
        "A's AboutTask edge only",
    );

    // B: its own Corrects row (distinct weight/event), no A row.
    let b_corrects = store
        .edges_by_rel(&b_root, RelKind::Corrects)
        .await
        .expect("b corrects");
    assert_edge_set(
        &b_corrects,
        &[edge("m", "n", RelKind::Corrects, 0.9, "wb-mn")],
        "B's Corrects row is B's own",
    );

    // Cross-scope: a rel that B has nothing of returns empty for B.
    let b_about = store
        .edges_by_rel(&b_root, RelKind::AboutTask)
        .await
        .expect("b about");
    assert!(b_about.is_empty(), "B has no AboutTask rows: {b_about:?}");
}

// --- 5. weight (f64) and rel (enum spelling) round-trip losslessly. ---

#[tokio::test]
async fn graph_arm_field_roundtrip_lossless() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("edges.db");
    let path = path.to_str().expect("path");

    let store = TursoFactEdges::new(path).await.expect("open");
    let a_root = scope("ws-a", None, None);

    // Every rel variant plus edge-case f64 weights that must survive a
    // binary64 round-trip exactly.
    let batch = vec![
        edge("r1", "r2", RelKind::Supersedes, 0.1, "w1"),
        edge("r2", "r3", RelKind::DependsOn, 123456.789, "w2"),
        edge("r3", "r4", RelKind::AboutTask, 1e-9, "w3"),
        edge("r4", "r5", RelKind::MentionsFile, -0.5, "w4"),
        edge(
            "r5",
            "r6",
            RelKind::CausedBy,
            2.0_f64.powi(1023) * 0.25,
            "w5",
        ),
        edge("r6", "r7", RelKind::Corrects, f64::MIN_POSITIVE, "w6"),
        edge("r7", "r8", RelKind::Related, 0.0, "w7"),
    ];
    upsert(&store, &a_root, &batch).await;

    // Pull every edge back through a deep traversal (undirected) and compare
    // field-by-field, including exact f64 and the enum's stable spelling.
    let got = store
        .neighbors(&[FactId("r1".into())], 10, &a_root)
        .await
        .expect("neighbors");

    let want: Vec<Edge> = batch.clone();
    assert_eq!(got.len(), want.len(), "all seven edges returned: {got:?}");
    let mut by_event: BTreeSet<String> = got.iter().map(|e| e.source_event_id.clone()).collect();
    for want_edge in &want {
        let hit = got
            .iter()
            .find(|e| e.source_event_id == want_edge.source_event_id)
            .unwrap_or_else(|| panic!("missing round-trip edge {want_edge:?} in {got:?}"));
        by_event.remove(&want_edge.source_event_id);
        assert_eq!(
            &hit.src.0, &want_edge.src.0,
            "src: {hit:?} vs {want_edge:?}"
        );
        assert_eq!(
            &hit.dst.0, &want_edge.dst.0,
            "dst: {hit:?} vs {want_edge:?}"
        );
        assert_eq!(
            hit.rel, want_edge.rel,
            "rel spelling: {hit:?} vs {want_edge:?}"
        );
        assert_eq!(
            hit.weight, want_edge.weight,
            "f64 exact round-trip: {hit:?} vs {want_edge:?}"
        );
    }
    assert!(by_event.is_empty(), "no extra edges: {by_event:?}");
}
