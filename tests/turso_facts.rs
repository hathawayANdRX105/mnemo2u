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

    // An unknown id is a NotFound, not a silent no-op.
    let error = store
        .supersede(&FactId("missing".into()), &corrected)
        .await
        .expect_err("unknown id");
    assert!(
        matches!(error, mnemo2u::core::traits::StoreError::NotFound(_)),
        "{error}"
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
