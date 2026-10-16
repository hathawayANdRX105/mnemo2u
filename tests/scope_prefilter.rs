//! `MemoryFactStore` scope pre-filter contract (stage doc 03 contract 1):
//! every read applies `ScopeKey::admits` *inside* the store — a narrower or
//! foreign scope never sees another scope's rows, not even their ranks, and
//! `get` matches the scope exactly.

use mnemo2u::core::traits::{FactStore, StoreError};
use mnemo2u::core::types::{Fact, FactId, ScopeKey, TrustState};
use mnemo2u::store::facts::MemoryFactStore;

fn scope(workspace: &str, session: Option<&str>, task: Option<&str>) -> ScopeKey {
    ScopeKey {
        workspace_id: workspace.to_string(),
        session_id: session.map(str::to_string),
        task_id: task.map(str::to_string),
    }
}

fn fact(id: &str, scope: &ScopeKey, text: &str) -> Fact {
    Fact {
        id: FactId(id.to_string()),
        scope: scope.clone(),
        kind: "semantic".to_string(),
        text: text.to_string(),
        source_refs: vec![],
        trust_state: TrustState::Candidate,
        valid_from: None,
        valid_to: None,
        created_at: 0,
        superseded_by: None,
        revision: 0,
    }
}

/// 1. Cross-workspace leakage is impossible: B's highly relevant text must
/// never appear in an A-scoped query.
#[tokio::test]
async fn cross_workspace_never_leaks() {
    let store = MemoryFactStore::new();
    let ws_a = scope("ws-a", None, None);
    let ws_b = scope("ws-b", None, None);

    let rows = vec![
        fact(
            "b-1",
            &ws_b,
            "the compiler emits a diagnostic about each broken borrow",
        ),
        fact("a-1", &ws_a, "the compiler has a broken config flag"),
    ];
    for row in &rows {
        store.put(row).await.expect("put");
    }

    let hits = store
        .search(&ws_a, "compiler diagnostic borrow broken", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert!(
        hit_ids.iter().any(|id| id == "a-1"),
        "A row scores: {hit_ids:?}"
    );
    assert!(
        !hit_ids.iter().any(|id| id == "b-1"),
        "B row leaked into an A-scoped query: {hit_ids:?}"
    );

    // The mirror direction: B-scoped queries see B only.
    let hits = store
        .search(&ws_b, "compiler diagnostic borrow broken", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["b-1".to_string()]);
}

/// 2. A session constraint narrows recall to that session; an unconstrained
/// session admits everything in the workspace (`admits` semantics).
#[tokio::test]
async fn session_scope_narrows_recall() {
    let store = MemoryFactStore::new();
    let base = scope("ws-n", None, None);
    let s1 = scope("ws-n", Some("s1"), None);
    let s2 = scope("ws-n", Some("s2"), None);

    store
        .put(&fact("n-1", &s1, "alpha beta gamma delta"))
        .await
        .expect("put s1");
    store
        .put(&fact("n-2", &s2, "alpha beta gamma epsilon"))
        .await
        .expect("put s2");

    // s1 sees only its own row, even though its text is queried most fully.
    let hits = store
        .search(&s1, "alpha beta gamma delta", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["n-1".to_string()]);

    // Unconstrained: both rows, tied on score, ordered by id.
    let hits = store
        .search(&base, "alpha beta gamma", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["n-1".to_string(), "n-2".to_string()]);
}

/// 3. Same narrowing by task constraint.
#[tokio::test]
async fn task_scope_narrows_recall() {
    let store = MemoryFactStore::new();
    let base = scope("ws-t", None, None);
    let t1 = scope("ws-t", None, Some("t1"));
    let t2 = scope("ws-t", None, Some("t2"));

    store
        .put(&fact("t-1", &t1, "alpha beta gamma delta"))
        .await
        .expect("put t1");
    store
        .put(&fact("t-2", &t2, "alpha beta gamma epsilon"))
        .await
        .expect("put t2");

    let hits = store
        .search(&t1, "alpha beta gamma delta", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["t-1".to_string()]);

    let hits = store
        .search(&base, "alpha beta gamma", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["t-1".to_string(), "t-2".to_string()]);
}

/// 4. `get` is exact-scope: a foreign, different or merely narrower scope
/// gets `None` for a stored id.
#[tokio::test]
async fn get_requires_exact_scope() {
    let store = MemoryFactStore::new();
    let owner = scope("ws-g", Some("s1"), None);
    let row = fact("g-1", &owner, "gadget");
    store.put(&row).await.expect("put");

    let fetched = store
        .get(&owner, &row.id)
        .await
        .expect("get")
        .expect("owner scope sees its row");
    assert_eq!(fetched.id, row.id);
    assert_eq!(fetched.scope, row.scope);
    assert_eq!(fetched.text, row.text);

    assert!(
        store
            .get(&scope("ws-g", Some("s2"), None), &row.id)
            .await
            .expect("get")
            .is_none(),
        "different session in the same workspace"
    );
    assert!(
        store
            .get(&scope("ws-g", None, None), &row.id)
            .await
            .expect("get")
            .is_none(),
        "a wider scope does not widen get"
    );
    assert!(
        store
            .get(&scope("ws-other", None, None), &row.id)
            .await
            .expect("get")
            .is_none(),
        "foreign workspace"
    );
}

/// 5. `supersede` retires the old row from recall but keeps it queryable,
/// and a missing old id is a hard `NotFound` that stores nothing.
#[tokio::test]
async fn supersede_retires_old_and_keeps_marked_row() {
    let store = MemoryFactStore::new();
    let scope = scope("ws-s", Some("s1"), None);

    let mut old = fact("f-old", &scope, "gadget registry default timeout");
    old.revision = 5;
    store.put(&old).await.expect("put old");
    let mut new = fact("f-new", &scope, "gadget registry explicit timeout");
    new.created_at = 1_700;

    store.supersede(&old.id, &new).await.expect("supersede");

    // Old leaves search; new takes its place.
    let hits = store
        .search(&scope, "gadget registry timeout", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["f-new".to_string()]);

    // Old is still retrievable, marked, and its revision is bumped by one.
    let marked = store
        .get(&scope, &old.id)
        .await
        .expect("get old")
        .expect("old row");
    assert_eq!(marked.superseded_by, Some(FactId("f-new".to_string())));
    assert_eq!(marked.revision, 6);
    assert_eq!(
        marked.valid_to,
        Some(new.created_at),
        "the correction closes the validity window"
    );
    assert_eq!(marked.text, old.text, "the row keeps its history");

    // A missing old id errors and stores nothing.
    let ghost = fact("f-ghost", &scope, "gadget registry ghost timeout");
    let err = store
        .supersede(&FactId("f-missing".to_string()), &ghost)
        .await
        .expect_err("supersede must fail on missing old id");
    assert!(matches!(err, StoreError::NotFound(_)));
    assert!(store
        .get(&scope, &ghost.id)
        .await
        .expect("get ghost")
        .is_none());
    let hits = store
        .search(&scope, "gadget registry ghost timeout", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert!(!hit_ids.contains(&ghost.id.0));
}

/// A row closed only by `valid_to` (an invalidation without a replacement)
/// leaves search exactly like a superseded one — the temporal half of
/// `Fact::is_current`; closure is not deletion.
#[tokio::test]
async fn valid_to_closed_rows_leave_search() {
    let store = MemoryFactStore::new();
    let scope = scope("ws-v", None, None);

    let live = fact("v-live", &scope, "gadget registry default timeout");
    let closed = {
        let mut row = fact("v-closed", &scope, "gadget registry default timeout closed");
        row.valid_to = Some(999);
        row
    };
    store.put(&live).await.expect("put live");
    store.put(&closed).await.expect("put closed");

    let hits = store
        .search(&scope, "gadget registry timeout", 10)
        .await
        .expect("search");
    let hit_ids: Vec<String> = hits.iter().map(|(id, _)| id.0.clone()).collect();
    assert_eq!(hit_ids, vec!["v-live".to_string()]);

    let row = store
        .get(&scope, &closed.id)
        .await
        .expect("get")
        .expect("row");
    assert!(!row.is_current());
}

/// `scan` is the truth-surface enumeration: every admitted row in stable id
/// order, closed rows included, foreign scopes excluded.
#[tokio::test]
async fn scan_returns_every_admitted_row_in_id_order() {
    let store = MemoryFactStore::new();
    let key = scope("ws-scan", None, None);
    let other = scope("ws-other", None, None);

    store.put(&fact("s-3", &key, "third")).await.expect("put");
    store.put(&fact("s-1", &key, "first")).await.expect("put");
    let mut closed = fact("s-2", &key, "second");
    closed.valid_to = Some(9);
    store.put(&closed).await.expect("put");
    store
        .put(&fact("x-9", &other, "foreign"))
        .await
        .expect("put");

    let rows = store.scan(&key).await.expect("scan");
    let ids: Vec<String> = rows.iter().map(|row| row.id.0.clone()).collect();
    assert_eq!(
        ids,
        vec!["s-1", "s-2", "s-3"],
        "stable id order, closed row included, foreign scope excluded"
    );
}

/// 6. `limit` truncates the low-score tail; ties keep a stable
/// id-lexicographic order across calls.
#[tokio::test]
async fn limit_truncates_low_scores_and_ties_are_stable() {
    let store = MemoryFactStore::new();
    let scope = scope("ws-l", None, None);

    // "aaa" and "alpha" share all three query tokens (1.0), and "low" only
    // one (~0.33) — so top-2 keeps the tied pair and drops the low scorer.
    for row in [
        fact("aaa", &scope, "one two three four five"),
        fact("alpha", &scope, "one two three four five"),
        fact("low", &scope, "one unrelated zzz"),
    ] {
        store.put(&row).await.expect("put");
    }

    fn hit_ids(hits: &[(FactId, f64)]) -> Vec<String> {
        hits.iter().map(|(id, _)| id.0.clone()).collect()
    }

    let query = "one two three";
    assert_eq!(
        hit_ids(&store.search(&scope, query, 2).await.expect("search top-2")),
        vec!["aaa".to_string(), "alpha".to_string()]
    );
    assert_eq!(
        hit_ids(&store.search(&scope, query, 1).await.expect("search top-1")),
        vec!["aaa".to_string()],
        "top-1 takes the lexicographically first tie"
    );
    assert_eq!(
        hit_ids(&store.search(&scope, query, 10).await.expect("search full")),
        vec!["aaa".to_string(), "alpha".to_string(), "low".to_string()],
        "full result keeps the low scorer last"
    );

    // The sequence is deterministic: two identical calls agree.
    let first = store.search(&scope, query, 10).await.expect("search first");
    let second = store
        .search(&scope, query, 10)
        .await
        .expect("search second");
    assert_eq!(hit_ids(&first), hit_ids(&second));
}

/// 7. The audit ledger appends in call order and is snapshotable while rows
/// churn around it.
#[tokio::test]
async fn audit_ledger_appends_in_order() {
    let store = MemoryFactStore::new();
    let scope = scope("ws-u", None, None);

    store
        .put(&fact("u-1", &scope, "audit row one"))
        .await
        .expect("put");
    store.append_audit("put", "sha:u-1").await.expect("audit");
    let new = fact("u-2", &scope, "audit row two");
    store
        .supersede(&FactId("u-1".to_string()), &new)
        .await
        .expect("supersede");
    store
        .append_audit("supersede", "sha:u-2")
        .await
        .expect("audit");

    assert_eq!(
        store.audit(),
        vec![
            ("put".to_string(), "sha:u-1".to_string()),
            ("supersede".to_string(), "sha:u-2".to_string())
        ]
    );
}
