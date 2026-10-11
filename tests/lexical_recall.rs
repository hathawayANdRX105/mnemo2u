//! Turso FTS lexical arm contract (stage doc 03 T3): exact-identifier recall,
//! in-query scope pre-filtering, drop/replace semantics, and sanitized
//! queries.
//!
//! One database file, two workspaces, twenty-six facts: the fixture mirrors
//! what an encoded session stream looks like — near-duplicate prose with
//! exact identifiers (function spellings, paths, coverage bundles) that only
//! the lexical arm can match.

use std::collections::BTreeSet;

use mnemo2u::core::traits::LexicalStore;
use mnemo2u::core::types::{FactId, ScopeKey};
use mnemo2u::store::turso_lexical::TursoLexical;

/// One indexed fact: the row's storage scope and its text.
#[derive(Clone)]
struct Row {
    id: &'static str,
    scope: ScopeKey,
    text: &'static str,
}

fn scope(ws: &str, sess: Option<&str>, task: Option<&str>) -> ScopeKey {
    ScopeKey {
        workspace_id: ws.to_string(),
        session_id: sess.map(str::to_string),
        task_id: task.map(str::to_string),
    }
}

/// Workspace A — 14 facts spread over four (session, task) combinations.
fn fixture_a() -> Vec<Row> {
    let a = |sess: Option<&str>, task: Option<&str>| scope("ws-a", sess, task);
    vec![
        // (ws A, unconstrained scope) — one exact identifier each.
        Row {
            id: "f-auth",
            scope: a(None, None),
            text: "gateway rejected the auth call with CheckAuthError(401) and recorded the trace",
        },
        Row {
            id: "f-path",
            scope: a(None, None),
            text: "the lexical arm lives at src/store/turso_lexical.rs and owns the fts index",
        },
        Row {
            id: "f-cov",
            scope: a(None, None),
            text: "the coverage bundle cov-dot--ferrite wraps the ferrite dot report",
        },
        Row {
            id: "f-region",
            scope: a(None, None),
            text: "RegionBudget::from_context_window derives the token budget for recall",
        },
        Row {
            id: "f-audit",
            scope: a(None, None),
            text: "the repair loop calls append_audit after every supersede",
        },
        // (ws A, session s1, unconstrained task)
        Row {
            id: "f-kelpa",
            scope: a(Some("s1"), None),
            text: "kelpite shelf a holds the staged notes",
        },
        Row {
            id: "f-kelpb",
            scope: a(Some("s1"), None),
            text: "kelpite shelf b holds the staged drafts",
        },
        Row {
            id: "f-plumb",
            scope: a(Some("s1"), None),
            text: "plumite bench b is the second staging area",
        },
        // (ws A, session s1, task t1)
        Row {
            id: "f-kelpc",
            scope: a(Some("s1"), Some("t1")),
            text: "kelpite shelf c holds the verified claims",
        },
        Row {
            id: "f-sigil",
            scope: a(Some("s1"), Some("t1")),
            text: "sigilite forge is where the claims are cast",
        },
        Row {
            id: "f-marble",
            scope: a(Some("s1"), Some("t1")),
            text: "marble quarry backs up the cast claims",
        },
        // (ws A, session s2, unconstrained task)
        Row {
            id: "f-quartz",
            scope: a(Some("s2"), None),
            text: "quartzite slab a keeps the open threads",
        },
        Row {
            id: "f-onyx",
            scope: a(Some("s2"), None),
            text: "onyxite vein b keeps the open threads",
        },
        Row {
            id: "f-slate",
            scope: a(Some("s2"), None),
            text: "slatite roof c keeps the open threads",
        },
    ]
}

/// Workspace B — 12 facts, unconstrained scope. Several mirror A's most
/// query-relevant content so the cross-workspace negation is not vacuous.
fn fixture_b() -> Vec<Row> {
    let b = |sess: Option<&str>, task: Option<&str>| scope("ws-b", sess, task);
    vec![
        Row {
            id: "f-bauth",
            scope: b(None, None),
            text: "checkin service logged CheckAuthError(401) at the edge gateway",
        },
        Row {
            id: "f-bpath",
            scope: b(None, None),
            text: "the migration moved src/store/turso_lexical.rs into the backend tree",
        },
        Row {
            id: "f-bcov",
            scope: b(None, None),
            text: "the coverage gate cov-dot--ferrite blocks the release",
        },
        Row {
            id: "f-bregion",
            scope: b(None, None),
            text: "RegionBudget::from_context_window was tuned to eight thousand tokens",
        },
        Row {
            id: "f-baudit",
            scope: b(None, None),
            text: "append_audit writes to the journal, not to the memory",
        },
        Row {
            id: "f-bfill1",
            scope: b(None, None),
            text: "barite cave one stores the cold archives",
        },
        Row {
            id: "f-bfill2",
            scope: b(None, None),
            text: "dolomite mine two stores the cold archives",
        },
        Row {
            id: "f-bfill3",
            scope: b(None, None),
            text: "garnet kiln three stores the cold archives",
        },
        Row {
            id: "f-bfill4",
            scope: b(None, None),
            text: "mica shed four stores the cold archives",
        },
        Row {
            id: "f-bfill5",
            scope: b(None, None),
            text: "obsidian forge five stores the cold archives",
        },
        Row {
            id: "f-bfill6",
            scope: b(None, None),
            text: "the warehouse keeps garnet kiln spares on the shelf",
        },
        Row {
            id: "f-bfill7",
            scope: b(None, None),
            text: "the warehouse keeps obsidian forge spares on the shelf",
        },
    ]
}

/// Index `rows` into `store`, one call per row, under each row's own scope.
async fn index_all(store: &TursoLexical, rows: &[Row]) {
    for row in rows {
        store
            .index(&row.scope, &FactId(row.id.to_string()), row.text)
            .await
            .expect("index");
    }
}

/// The ids in a search result, in order — used for top-hit checks.
fn ids(hits: &[(FactId, f64)]) -> Vec<String> {
    hits.iter().map(|(id, _)| id.0.clone()).collect()
}

/// A search result must never carry the same id twice.
fn assert_distinct(hits: &[(FactId, f64)]) {
    let unique: BTreeSet<&str> = hits.iter().map(|(id, _)| id.0.as_str()).collect();
    assert_eq!(
        unique.len(),
        hits.len(),
        "duplicate ids in one search result: {unique:?}"
    );
}

#[tokio::test]
async fn lexical_identifier_recall_scope_pre_filter_and_lifecycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("lexical.db");
    let path = path.to_str().expect("path");

    let a = fixture_a();
    let b = fixture_b();
    let store = TursoLexical::new(path).await.expect("open");
    index_all(&store, &a).await;
    index_all(&store, &b).await;

    let b_scope = scope("ws-b", None, None);
    let a_root = scope("ws-a", None, None);

    // --- 1. Exact-identifier recall: the top hit contains the identifier. ---
    let auth = store
        .search(&a_root, "CheckAuthError(401)", 10)
        .await
        .expect("search auth");
    assert!(
        ids(&auth).first() == Some(&"f-auth".to_string()),
        "top hit for the identifier query: {auth:?}"
    );

    let path_hits = store
        .search(&a_root, "src/store/turso_lexical.rs", 10)
        .await
        .expect("search path");
    assert!(
        ids(&path_hits).first() == Some(&"f-path".to_string()),
        "top hit for the file path query: {path_hits:?}"
    );

    // The other exact identifiers in the fixture recall their own fact.
    for (query, expected) in [
        ("cov-dot--ferrite", "f-cov"),
        ("RegionBudget::from_context_window", "f-region"),
        ("append_audit", "f-audit"),
    ] {
        let hits = store.search(&a_root, query, 5).await.expect("search");
        assert!(
            ids(&hits).first() == Some(&expected.to_string()),
            "top hit for {query:?}: {hits:?}"
        );
    }

    // --- 2. Cross-workspace negation. ---
    // B's most query-relevant facts (f-bauth, f-bpath) exist; a search in A's
    // scope must not surface any B id.
    let b_ids: BTreeSet<&str> = b.iter().map(|row| row.id).collect();
    for query in [
        "CheckAuthError(401)",
        "src/store/turso_lexical.rs",
        "cov-dot--ferrite",
        "RegionBudget::from_context_window",
        "append_audit",
    ] {
        let hits = store.search(&a_root, query, 20).await.expect("search");
        let leaked: Vec<String> = ids(&hits)
            .into_iter()
            .filter(|id| b_ids.contains(id.as_str()))
            .collect();
        assert!(
            leaked.is_empty(),
            "A-scope search for {query:?} leaked B ids: {leaked:?}"
        );
    }

    // B's scope sees its own most-relevant fact at the top — the negation is
    // real only if B's content would otherwise win.
    let b_auth = store
        .search(&b_scope, "CheckAuthError(401)", 10)
        .await
        .expect("search b");
    assert!(
        ids(&b_auth).first() == Some(&"f-bauth".to_string()),
        "B-scope top hit: {b_auth:?}"
    );

    // --- 3. Session/task narrowing matches ScopeKey::admits. ---
    // "kelpite" is held by f-kelpa / f-kelpb (session s1, task unconstrained)
    // and f-kelpc (session s1, task t1). For each query scope, the visible
    // set must equal the admits-filtered set.
    let kelp: Vec<&Row> = a
        .iter()
        .filter(|row| row.text.contains("kelpite"))
        .collect();
    let narrow_scopes: [ScopeKey; 5] = [
        a_root.clone(),
        scope("ws-a", Some("s1"), None),
        scope("ws-a", Some("s1"), Some("t1")),
        scope("ws-a", Some("s2"), None),
        scope("ws-a", Some("s3"), None),
    ];
    for query_scope in &narrow_scopes {
        let expected: Vec<String> = kelp
            .iter()
            .filter(|row| query_scope.admits(&row.scope))
            .map(|row| row.id.to_string())
            .collect();
        let hits = store
            .search(query_scope, "kelpite", 10)
            .await
            .expect("search kelpite");
        assert_distinct(&hits);
        let mut got = ids(&hits);
        got.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "kelpite visibility under scope {query_scope:?} (expected {expected:?})"
        );
    }

    // An unconstrained task admits the whole session: s1 without a task sees
    // all three kelpite rows; s1 + t1 only the task-owned one; s2 none.
    let s1 = store
        .search(&scope("ws-a", Some("s1"), None), "kelpite", 10)
        .await
        .expect("search s1");
    assert_eq!(
        s1.len(),
        3,
        "s1 (no task) must admit the task-owned row: {s1:?}"
    );
    let s1t1 = store
        .search(&scope("ws-a", Some("s1"), Some("t1")), "kelpite", 10)
        .await
        .expect("search s1 t1");
    assert_eq!(s1t1.len(), 1, "s1+t1 must narrow to the task row: {s1t1:?}");
    let s2 = store
        .search(&scope("ws-a", Some("s2"), None), "kelpite", 10)
        .await
        .expect("search s2");
    assert!(
        s2.is_empty(),
        "another session must not see s1's rows: {s2:?}"
    );

    // top_k truncates the same query.
    let truncated = store.search(&a_root, "kelpite", 2).await.expect("search");
    assert_eq!(truncated.len(), 2, "top_k must truncate: {truncated:?}");

    // --- 4. Drop removes; re-index replaces instead of duplicating. ---
    let kelp_all = store.search(&a_root, "kelpite", 10).await.expect("search");
    assert_eq!(kelp_all.len(), 3, "baseline: {kelp_all:?}");

    let kelpc = a.iter().find(|row| row.id == "f-kelpc").expect("kelpc");
    store
        .drop(&kelpc.scope, &FactId(kelpc.id.into()))
        .await
        .expect("drop");
    let after_drop = store
        .search(&a_root, "kelpite", 10)
        .await
        .expect("search after drop");
    assert_eq!(
        ids(&after_drop),
        ["f-kelpa".to_string(), "f-kelpb".to_string()],
        "kelpc must be gone: {after_drop:?}"
    );
    // Dropping the same (scope, id) again is not an error.
    store
        .drop(&kelpc.scope, &FactId(kelpc.id.into()))
        .await
        .expect("drop again");

    // Replace f-kelpa's text: the old marker stops matching, the new one
    // starts, and the result still carries no duplicate id.
    let kelpa = a.iter().find(|row| row.id == "f-kelpa").expect("kelpa");
    store
        .index(
            &kelpa.scope,
            &FactId("f-kelpa".into()),
            "mica shelf a holds the staged notes",
        )
        .await
        .expect("reindex");
    let kelp_after = store.search(&a_root, "kelpite", 10).await.expect("search");
    assert_eq!(
        ids(&kelp_after),
        ["f-kelpb".to_string()],
        "kelpa must no longer match kelpite: {kelp_after:?}"
    );
    let mica = store.search(&a_root, "mica", 10).await.expect("search");
    assert!(
        ids(&mica).first() == Some(&"f-kelpa".to_string()),
        "kelpa must match its new text: {mica:?}"
    );
    assert_distinct(&mica);

    // --- 5. Malformed queries degrade, never error. ---
    for query in [
        "-",
        "\"\"",
        "AND",
        "AND OR NOT - \"",
        "\"",
        "a: b",
        "cov-dot--ferrite AND",
        "",
    ] {
        let hits = store
            .search(&a_root, query, 10)
            .await
            .expect("search {query:?}");
        assert_distinct(&hits);
    }

    // --- Reopening the file keeps serving recall (the index is on disk). ---
    drop(store);
    let reopened = TursoLexical::new(path).await.expect("reopen");
    let hits = reopened
        .search(&a_root, "CheckAuthError(401)", 10)
        .await
        .expect("search after reopen");
    assert!(
        ids(&hits).first() == Some(&"f-auth".to_string()),
        "reopen lost recall: {hits:?}"
    );
}
