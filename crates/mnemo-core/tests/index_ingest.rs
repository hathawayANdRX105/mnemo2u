//! Session ingest — turn rows, the same-call watermark commit and replay
//! safety (`src/index/ingest.rs`).

use std::sync::Arc;

use mnemo2u::index::ingest::{SessionIngest, SessionTurn, Turn};
use mnemo2u::store::memory::MemoryKv;

fn turn(id: &str, role: &str, content: &str, timestamp: i64) -> SessionTurn {
    SessionTurn {
        conversation_id: "conv-1".to_string(),
        turn: Turn {
            turn_id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp,
        },
    }
}

fn ingest() -> SessionIngest {
    SessionIngest::new(
        Arc::new(MemoryKv::new()),
        Arc::new(MemoryKv::new()),
        Arc::new(MemoryKv::new()),
    )
}

#[tokio::test]
async fn rows_are_deterministic_per_turn() {
    let ingest = ingest();
    let turns = vec![
        turn("t1", "user", "hello", 1),
        turn("t2", "assistant", "hi", 2),
    ];
    let first = ingest.rows_for_turns(&turns);
    let second = ingest.rows_for_turns(&turns);
    assert_eq!(first.len(), 2);
    assert_eq!(
        first.iter().map(|(id, _, _)| id).collect::<Vec<_>>(),
        second.iter().map(|(id, _, _)| id).collect::<Vec<_>>(),
        "the same turn always maps to the same chunk id"
    );
    assert_ne!(first[0].0, first[1].0, "two turns are two chunks");
}

#[tokio::test]
async fn row_metadata_carries_conversation_and_path() {
    let ingest = ingest();
    let rows = ingest.rows_for_turns(&[turn("t1", "user", "hello", 7)]);
    let (chunk_id, doc_id, chunk) = &rows[0];
    assert_eq!(chunk.content, "hello");
    assert_eq!(chunk.full_doc_id, "conv-conv-1");
    assert_eq!(chunk.file_path, "conversation/conv-1");
    assert_eq!(doc_id, "conv-1");
    assert!(chunk_id.starts_with("turn-"), "chunk ids are namespaced");
}

#[tokio::test]
async fn commit_writes_rows_and_advances_the_watermark_in_one_call() {
    let ingest = ingest();
    let turns = vec![
        turn("t1", "user", "hello", 1),
        turn("t2", "assistant", "hi", 2),
    ];
    let written = ingest.commit_turns("conv-1", &turns).await.expect("commit");
    assert_eq!(written, 2);

    let watermark = ingest
        .watermark("conv-1")
        .await
        .expect("read")
        .expect("watermark present");
    assert_eq!(watermark.last_turn_id, "t2");
    assert_eq!(watermark.last_timestamp, 2);

    // The chunk rows the watermark claims are actually there.
    let rows = ingest.rows_for_turns(&turns);
    for (chunk_id, _, _) in &rows {
        assert!(
            ingest
                .text_chunks
                .get_by_id(chunk_id)
                .await
                .expect("read")
                .is_some(),
            "the watermark never runs ahead of the data"
        );
    }
}

#[tokio::test]
async fn replay_of_committed_turns_adds_nothing() {
    let ingest = ingest();
    let turns = vec![turn("t1", "user", "hello", 1)];
    assert_eq!(
        ingest.commit_turns("conv-1", &turns).await.expect("first"),
        1
    );
    assert_eq!(
        ingest.commit_turns("conv-1", &turns).await.expect("second"),
        0,
        "re-sending a committed turn is a no-op"
    );
}

#[tokio::test]
async fn pending_turns_returns_the_tail_after_the_watermark() {
    let ingest = ingest();
    let first = vec![turn("t1", "user", "hello", 1)];
    ingest.commit_turns("conv-1", &first).await.expect("commit");

    let all = vec![
        turn("t1", "user", "hello", 1),
        turn("t2", "assistant", "hi", 2),
        turn("t3", "user", "more", 3),
    ];
    let pending = ingest.pending_turns("conv-1", &all).await;
    assert_eq!(
        pending
            .iter()
            .map(|t| t.turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["t2", "t3"],
        "only the turns after the watermark are pending"
    );
}

#[tokio::test]
async fn watermark_naming_an_unknown_turn_commits_everything() {
    let ingest = ingest();
    ingest
        .watermarks
        .upsert(vec![(
            "conv-1".to_string(),
            serde_json::json!({"last_turn_id": "gone", "last_timestamp": 9}),
        )])
        .await
        .expect("seed watermark");

    let all = vec![
        turn("t1", "user", "hello", 1),
        turn("t2", "assistant", "hi", 2),
    ];
    let pending = ingest.pending_turns("conv-1", &all).await;
    assert_eq!(
        pending.len(),
        2,
        "an unknown watermark is the safe direction"
    );
}

#[tokio::test]
async fn turns_of_other_conversations_are_ignored() {
    let ingest = ingest();
    let mut foreign = turn("t9", "user", "other", 5);
    foreign.conversation_id = "conv-2".to_string();
    let turns = vec![turn("t1", "user", "hello", 1), foreign];
    let pending = ingest.pending_turns("conv-1", &turns).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].turn.turn_id, "t1");
}
