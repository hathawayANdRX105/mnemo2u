//! Durable `TursoFactStore` contract tests — the row semantics the recall
//! contracts depend on, proven on a real database rather than the in-memory
//! backend (stage doc 03 contract 1: the scope filter applies inside the
//! store, never as a post-filter over unscoped ranks).

use mnemo2u::core::traits::FactStore;
use mnemo2u::core::types::{Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::store::turso_facts::TursoFactStore;

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
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: "s1".to_string(),
            branch_id: "b1".to_string(),
            epoch: 1,
            event_id: "e1".to_string(),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: 1_700_000_000_000,
        superseded_by: None,
        revision: 0,
    }
}

fn temp_path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("mnemo2u-facts-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("facts.db").to_string_lossy().to_string()
}

#[tokio::test]
async fn put_then_get_round_trips_with_exact_scope() {
    let store = TursoFactStore::open(&temp_path("roundtrip"))
        .await
        .expect("open");
    let key = scope("ws-a", Some("s1"), Some("t1"));
    store
        .put(&fact("f1", &key, "turso is the truth layer"))
        .await
        .expect("put");

    let found = store.get(&key, &FactId("f1".into())).await.expect("get");
    assert_eq!(found.expect("row").text, "turso is the truth layer");

    // Exact scope: a narrower or wider request must not match.
    assert!(store
        .get(&scope("ws-a", None, None), &FactId("f1".into()))
        .await
        .expect("get wider")
        .is_none());
    assert!(store
        .get(&scope("ws-a", Some("s1"), None), &FactId("f1".into()))
        .await
        .expect("get without task")
        .is_none());
    assert!(store
        .get(&scope("ws-b", Some("s1"), Some("t1")), &FactId("f1".into()))
        .await
        .expect("get other workspace")
        .is_none());
}

#[tokio::test]
async fn search_pre_filters_scope_inside_the_query() {
    let store = TursoFactStore::open(&temp_path("scope"))
        .await
        .expect("open");
    let wide = scope("ws-a", None, None);
    let narrow = scope("ws-a", Some("s1"), Some("t1"));
    store
        .put(&fact("f1", &wide, "turso is the truth layer"))
        .await
        .expect("put wide");
    store
        .put(&fact("f2", &narrow, "lancedb stores vectors"))
        .await
        .expect("put narrow");

    // The wide scope admits both rows (the absent constraints admit
    // everything under the workspace).
    let hits = store
        .search(&wide, "turso lancedb", 10)
        .await
        .expect("search");
    assert_eq!(hits.len(), 2);

    // The narrow scope sees only its own row — and nothing from ws-b.
    store
        .put(&fact("f3", &scope("ws-b", None, None), "another workspace"))
        .await
        .expect("put other");
    let hits = store
        .search(&narrow, "turso lancedb workspace", 10)
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, FactId("f2".into()));

    // Negative case: a workspace that holds nothing must not leak a rank.
    let hits = store
        .search(&scope("ws-c", None, None), "turso", 10)
        .await
        .expect("search");
    assert!(hits.is_empty());
}

#[tokio::test]
async fn superseded_rows_leave_search_and_carry_the_chain() {
    let store = TursoFactStore::open(&temp_path("supersede"))
        .await
        .expect("open");
    let key = scope("ws-a", None, None);
    store
        .put(&fact("f1", &key, "the release ships on Friday"))
        .await
        .expect("put old");
    let mut corrected = fact("f2", &key, "the release ships on Monday");
    corrected.revision = 1;
    store
        .supersede(&FactId("f1".into()), &corrected)
        .await
        .expect("supersede");

    let hits = store
        .search(&key, "release ships", 10)
        .await
        .expect("search");
    assert_eq!(hits.len(), 1, "superseded row must leave search");
    assert_eq!(hits[0].0, FactId("f2".into()));

    // The old row survives with the chain and a bumped revision.
    let old = store
        .get(&key, &FactId("f1".into()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(old.superseded_by, Some(FactId("f2".into())));
    assert_eq!(old.revision, 1);
    assert_eq!(
        old.valid_to,
        Some(corrected.created_at),
        "the correction closes the validity window"
    );

    // An unknown id is a NotFound, not a silent no-op.
    let error = store
        .supersede(&FactId("missing".into()), &corrected)
        .await
        .expect_err("unknown id");
    assert!(
        matches!(error, mnemo2u::core::traits::StoreError::NotFound(_)),
        "{error}"
    );

    // A second correction of the same row stacks: the chain follows the
    // latest correction and the revision keeps counting. Both backends do
    // this unconditionally; the store has no anti-resurrection guard (the
    // gateway owns that decision).
    let third = fact("f3", &key, "the third release ships on friday");
    store.put(&third).await.expect("put third");
    store
        .supersede(&FactId("f1".into()), &third)
        .await
        .expect("supersede again");
    let old = store
        .get(&key, &FactId("f1".into()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(old.superseded_by, Some(FactId("f3".into())));
    assert_eq!(old.revision, 2);
    let hits = store
        .search(&key, "release ships", 10)
        .await
        .expect("search");
    assert_eq!(hits.len(), 2, "only the two active rows remain");
}

/// A row closed only by `valid_to` (an invalidation without a replacement)
/// leaves search — the durable backend enforces the same `Fact::is_current`
/// predicate the memory backend does.
#[tokio::test]
async fn valid_to_closed_rows_leave_search() {
    let store = TursoFactStore::open(&temp_path("valid-to"))
        .await
        .expect("open");
    let key = scope("ws-a", None, None);
    let live = fact("v-live", &key, "gadget registry default timeout");
    let mut closed = fact("v-closed", &key, "gadget registry default timeout closed");
    closed.valid_to = Some(999);
    store.put(&live).await.expect("put live");
    store.put(&closed).await.expect("put closed");

    let hits = store
        .search(&key, "gadget registry timeout", 10)
        .await
        .expect("search");
    let ids: Vec<FactId> = hits.into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, vec![FactId("v-live".into())]);

    // Closure is not deletion: the row still reads back by id.
    let row = store
        .get(&key, &FactId("v-closed".into()))
        .await
        .expect("get")
        .expect("row");
    assert!(!row.is_current());
}

/// `scan` is the truth-surface enumeration: every admitted row in stable id
/// order, closed rows included, foreign scopes excluded — the durable
/// backend mirrors the memory backend's contract.
#[tokio::test]
async fn scan_returns_every_admitted_row_in_id_order() {
    let store = TursoFactStore::open(&temp_path("scan"))
        .await
        .expect("open");
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

/// `supersede` locates the row by bare fact id, across scopes — the trait
/// takes no scope and `MemoryFactStore` does the same, so the two backends
/// stay interchangeable for a cross-scope correction.
#[tokio::test]
async fn supersede_locates_the_row_across_scopes() {
    let store = TursoFactStore::open(&temp_path("cross-scope"))
        .await
        .expect("open");
    let home = scope("ws-a", Some("s1"), None);
    let away = scope("ws-b", Some("s9"), Some("t9"));
    store
        .put(&fact("f1", &home, "the release ships on friday"))
        .await
        .expect("put");

    store
        .supersede(
            &FactId("f1".into()),
            &fact("f2", &away, "the release slips to monday"),
        )
        .await
        .expect("supersede");

    let old = store
        .get(&home, &FactId("f1".into()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(old.superseded_by, Some(FactId("f2".into())));
    assert_eq!(old.revision, 1);
    assert!(
        store
            .get(&away, &FactId("f1".into()))
            .await
            .expect("get")
            .is_none(),
        "the correction lands on the row that held the id, not on the scope"
    );
}

#[tokio::test]
async fn ranking_matches_the_memory_backend_ordering() {
    let store = TursoFactStore::open(&temp_path("ranking"))
        .await
        .expect("open");
    let key = scope("ws-a", None, None);
    store
        .put(&fact("f1", &key, "turso truth layer adjacency graph"))
        .await
        .expect("put");
    store
        .put(&fact("f2", &key, "lancedb vectors"))
        .await
        .expect("put");
    store
        .put(&fact("f3", &key, "turso turso turso"))
        .await
        .expect("put");

    let hits = store.search(&key, "turso", 10).await.expect("search");
    // f1 and f3 both share the only query token; the tie breaks on id.
    assert_eq!(hits[0].0, FactId("f1".into()));
    assert_eq!(hits[1].0, FactId("f3".into()));
    assert_eq!(hits[2].0, FactId("f2".into()));
    assert!(hits[2].1.abs() < f64::EPSILON, "unrelated row scores zero");
}

/// `put` must overwrite in place for every scope shape, including one with
/// any absent optional component.
///
/// A composite PRIMARY KEY containing a NULL column does not enforce
/// uniqueness (`NULL != NULL`), so binding `Option` straight through made
/// `INSERT OR REPLACE` a plain INSERT for those scopes: repeated puts
/// accumulated duplicate rows, `search` returned the same fact id more than
/// once, and the idempotent-upsert crash-recovery protocol could not
/// converge. Absent components are normalised to `""` on write instead
/// (`GatewayOptions::default()` is exactly the `(ws, None, None)` case).
#[tokio::test]
async fn put_is_idempotent_for_every_scope_shape() {
    let store = TursoFactStore::open(&temp_path("idempotent"))
        .await
        .expect("open");
    // The three shapes that break a naive PK: both absent, only the second
    // present, only the first present.
    let shapes = [
        scope("ws-a", None, None),
        scope("ws-a", Some("s1"), None),
        scope("ws-a", None, Some("t1")),
    ];
    for (index, key) in shapes.iter().enumerate() {
        let id = format!("f{index}");
        store.put(&fact(&id, key, "first text")).await.expect("put");
        // A second put of the same id with different content must replace,
        // not append.
        store
            .put(&fact(&id, key, "second text"))
            .await
            .expect("reput");

        let hits = store.search(key, "text", 10).await.expect("search");
        let rows: Vec<&FactId> = hits.iter().map(|(id, _)| id).collect();
        assert_eq!(
            rows,
            vec![&FactId(id.clone())],
            "one row per fact id under {key:?}, got {rows:?}"
        );
        let found = store
            .get(key, &FactId(id))
            .await
            .expect("get")
            .expect("row");
        assert_eq!(found.text, "second text", "the reput overwrote the body");
    }
}
