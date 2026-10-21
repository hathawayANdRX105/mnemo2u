//! `FactStore::put_many` — the batch write path.
//!
//! Two properties the single-row path gets for free and the batch path has
//! to keep:
//!
//! 1. **Same rows.** A batch write and the equivalent row-by-row writes end
//!    up with the same facts — same fields, same order out of `scan`.
//! 2. **All or nothing.** A batch that fails part-way leaves nothing behind.
//!    The single-row path gets this from autocommit; the batch path has to
//!    open a transaction to get it.
//!
//! The batch exists because the single-row path commits per row, and on a
//! single-writer engine a thousand rows are a thousand queued transactions.

use mnemo2u::core::traits::FactStore;
use mnemo2u::core::types::{Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::store::facts::MemoryFactStore;
use mnemo2u::store::turso_facts::TursoFactStore;

fn scope(workspace: &str) -> ScopeKey {
    ScopeKey {
        workspace_id: workspace.to_string(),
        session_id: None,
        task_id: None,
    }
}

fn fact(id: &str, scope: &ScopeKey, text: &str) -> Fact {
    Fact {
        id: FactId(id.to_string()),
        scope: scope.clone(),
        kind: "episodic".to_string(),
        text: text.to_string(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: "s1".to_string(),
            branch_id: "b1".to_string(),
            epoch: 1,
            event_id: format!("event-{id}"),
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
    let dir = std::env::temp_dir().join(format!("mnemo2u-batch-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("facts.db").to_string_lossy().to_string()
}

fn facts(scope: &ScopeKey) -> Vec<Fact> {
    (0..8)
        .map(|index| {
            fact(
                &format!("fact-{index}"),
                scope,
                &format!("第 {index} 条事实"),
            )
        })
        .collect()
}

/// The batch and the row-by-row writes leave the same rows behind.
#[tokio::test]
async fn a_batch_writes_the_same_rows_as_row_by_row_writes() {
    let scope = scope("batch-eq");
    let rows = facts(&scope);

    let batched = TursoFactStore::open(&temp_path("eq-batch"))
        .await
        .expect("open");
    batched.put_many(&rows).await.expect("batch write");

    let one_by_one = TursoFactStore::open(&temp_path("eq-single"))
        .await
        .expect("open");
    for row in &rows {
        one_by_one.put(row).await.expect("single write");
    }

    let from_batch = batched.scan(&scope).await.expect("scan");
    let from_single = one_by_one.scan(&scope).await.expect("scan");
    assert_eq!(
        from_batch.len(),
        from_single.len(),
        "batch and row-by-row must leave the same number of facts"
    );
    assert_eq!(from_batch.len(), rows.len());
    // `Fact` carries no `PartialEq`, so the comparison is field by field.
    for (batched, single) in from_batch.iter().zip(from_single.iter()) {
        assert_eq!(batched.id, single.id, "fact id");
        assert_eq!(batched.scope, single.scope, "scope");
        assert_eq!(batched.kind, single.kind, "kind");
        assert_eq!(batched.text, single.text, "text");
        assert_eq!(batched.source_refs, single.source_refs, "source refs");
        assert_eq!(batched.trust_state, single.trust_state, "trust state");
        assert_eq!(batched.valid_from, single.valid_from, "valid_from");
        assert_eq!(batched.valid_to, single.valid_to, "valid_to");
        assert_eq!(batched.created_at, single.created_at, "created_at");
        assert_eq!(batched.superseded_by, single.superseded_by, "superseded_by");
        assert_eq!(batched.revision, single.revision, "revision");
    }
}

/// A repeated id inside one batch behaves as repeated `put` calls would:
/// the last write wins.
#[tokio::test]
async fn a_repeated_id_inside_a_batch_keeps_the_last_write() {
    let scope = scope("batch-repeat");
    let store = TursoFactStore::open(&temp_path("repeat"))
        .await
        .expect("open");

    let first = fact("same-id", &scope, "第一版");
    let second = fact("same-id", &scope, "第二版");
    store.put_many(&[first, second]).await.expect("batch write");

    let row = store
        .get(&scope, &FactId("same-id".to_string()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.text, "第二版", "后写的覆盖先写的");
    assert_eq!(store.scan(&scope).await.expect("scan").len(), 1);
}

/// A batch that fails part-way leaves nothing behind.
#[tokio::test]
async fn a_failed_batch_leaves_nothing_behind() {
    let scope = scope("batch-atomic");
    let store = TursoFactStore::open(&temp_path("atomic"))
        .await
        .expect("open");
    let rows = facts(&scope);

    // The failure has to be one the database itself raises, so the row
    // before it is a real write that the rollback has to undo. `revision`
    // is a `u64` in Rust but an `INTEGER` column in SQL, so a value past
    // `i64::MAX` is what the driver rejects.
    let mut poisoned = rows.clone();
    poisoned[3].revision = u64::MAX;
    poisoned[3].text = "触发失败的那一行".to_string();

    let outcome = store.put_many(&poisoned).await;
    assert!(outcome.is_err(), "这一批应该失败");
    assert_eq!(
        store.scan(&scope).await.expect("scan").len(),
        0,
        "失败的一批不该留下任何行"
    );
}

/// An empty batch is a no-op, not an error.
#[tokio::test]
async fn an_empty_batch_writes_nothing_and_does_not_fail() {
    let scope = scope("batch-empty");
    let store = TursoFactStore::open(&temp_path("empty"))
        .await
        .expect("open");
    store.put_many(&[]).await.expect("empty batch");
    assert!(store.scan(&scope).await.expect("scan").is_empty());
}

/// The in-memory backend has the same contract: same rows, and a repeated
/// id inside a batch still resolves to the last write.
#[tokio::test]
async fn the_memory_backend_takes_the_same_batch() {
    let scope = scope("batch-memory");
    let store = MemoryFactStore::new();
    let rows = facts(&scope);

    store.put_many(&rows).await.expect("batch write");
    assert_eq!(store.scan(&scope).await.expect("scan").len(), rows.len());

    let mut repeated = rows.clone();
    repeated[0].text = "改过的".to_string();
    store.put_many(&repeated).await.expect("batch write");
    let row = store
        .get(&scope, &FactId("fact-0".to_string()))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.text, "改过的");
}
