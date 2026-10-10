//! Session ingest — closed-turn adapter for conversation streams (R2 T10).
//!
//! A harness streams turns; the R1 insert path consumes doc/chunk rows. This
//! module is the adapter between the two:
//!
//! - each turn becomes one chunk (no token windowing — a turn is already the
//!   unit the harness reasons about), carrying `conversation_id`, a role, and
//!   the turn's wall-clock time;
//! - the `organized_through` watermark records the last turn whose derived
//!   writes were committed, so a crash mid-stream resumes from the first
//!   uncommitted turn instead of re-extracting everything;
//! - the watermark and the truth rows share one commit call, so a crash can
//!   never leave the watermark ahead of the data it claims covers.
//!
//! The watermark lives in the truth layer (`KvStore`), not in a side file: the
//! repo's commit protocol keeps turso the only authority.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::rag::{Chunk, DocRecord};
use crate::core::text::compute_mdhash_id;
use crate::core::traits::{KvStore, Result as StoreResult};

/// KV namespace for the per-conversation watermark rows.
pub const NS_ORGANIZED_THROUGH: &str = "organized_through";

/// One closed turn from the harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// Stable id inside the conversation; the dedup key for replay.
    pub turn_id: String,
    /// `user` / `assistant` / `tool` … mirrored into the chunk metadata.
    pub role: String,
    pub content: String,
    /// Unix seconds when the turn closed.
    pub timestamp: i64,
}

/// A turn plus the conversation it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTurn {
    pub conversation_id: String,
    #[serde(flatten)]
    pub turn: Turn,
}

/// The watermark: the last turn id whose derived writes are committed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizedThrough {
    pub last_turn_id: String,
    pub last_timestamp: i64,
}

/// Session ingestion surface over the R1 truth stores.
pub struct SessionIngest {
    pub full_docs: Arc<dyn KvStore>,
    pub text_chunks: Arc<dyn KvStore>,
    pub watermarks: Arc<dyn KvStore>,
}

impl SessionIngest {
    pub fn new(
        full_docs: Arc<dyn KvStore>,
        text_chunks: Arc<dyn KvStore>,
        watermarks: Arc<dyn KvStore>,
    ) -> Self {
        Self {
            full_docs,
            text_chunks,
            watermarks,
        }
    }

    /// Read the watermark for `conversation_id` (absent = nothing committed).
    pub async fn watermark(&self, conversation_id: &str) -> StoreResult<Option<OrganizedThrough>> {
        Ok(self
            .watermarks
            .get_by_id(conversation_id)
            .await?
            .and_then(|row| serde_json::from_value::<OrganizedThrough>(row).ok()))
    }

    /// Turn a closed-turn stream into the doc/chunk rows for one insert.
    ///
    /// Already-known turn ids are skipped (replay safety), so re-sending a
    /// range the harness already delivered adds nothing.
    pub fn rows_for_turns(&self, turns: &[SessionTurn]) -> Vec<(String, String, Chunk)> {
        let mut rows = Vec::new();
        for turn in turns {
            let chunk_id = compute_mdhash_id(
                &format!("{}\u{1}{}", turn.conversation_id, turn.turn.turn_id),
                "turn-",
            );
            rows.push((
                chunk_id,
                turn.conversation_id.clone(),
                Chunk {
                    tokens: 0,
                    content: turn.turn.content.clone(),
                    chunk_order_index: 0,
                    full_doc_id: format!("conv-{}", turn.conversation_id),
                    file_path: format!("conversation/{}", turn.conversation_id),
                },
            ));
        }
        rows
    }

    /// Commit the truth rows for one batch of turns **together with** the
    /// watermark advance: same call, same failure mode, so the watermark can
    /// never claim data that is not there.
    pub async fn commit_turns(
        &self,
        conversation_id: &str,
        turns: &[SessionTurn],
    ) -> StoreResult<usize> {
        let rows = self.rows_for_turns(turns);
        if rows.is_empty() {
            return Ok(0);
        }
        let unknown = self
            .text_chunks
            .filter_keys(&rows.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>())
            .await?;
        let fresh: Vec<&(String, String, Chunk)> = rows
            .iter()
            .filter(|(id, _, _)| unknown.contains(id))
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }

        let doc_ids: Vec<String> = fresh
            .iter()
            .map(|(_, doc_id, _)| doc_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let doc_rows: Vec<(String, Value)> = doc_ids
            .iter()
            .map(|doc_id| {
                (
                    doc_id.clone(),
                    json!(DocRecord {
                        content: String::new(),
                    }),
                )
            })
            .collect();

        let chunk_rows: Vec<(String, Value)> = fresh
            .iter()
            .map(|(chunk_id, _, chunk)| {
                (
                    chunk_id.clone(),
                    serde_json::to_value(chunk).expect("chunk serializes"),
                )
            })
            .collect();

        let watermark = OrganizedThrough {
            last_turn_id: turns
                .last()
                .map(|turn| turn.turn.turn_id.clone())
                .unwrap_or_default(),
            last_timestamp: turns
                .last()
                .map(|turn| turn.turn.timestamp)
                .unwrap_or_default(),
        };

        // One ordered commit: docs, chunks, then the watermark. A failure
        // between them leaves the watermark behind the data, never ahead, so
        // the next run re-drives the same range.
        self.full_docs.upsert(doc_rows).await?;
        self.text_chunks.upsert(chunk_rows).await?;
        self.watermarks
            .upsert(vec![(
                conversation_id.to_string(),
                serde_json::to_value(&watermark).expect("watermark serializes"),
            )])
            .await?;
        Ok(fresh.len())
    }

    /// The turns that still need committing for a conversation: the tail after
    /// the watermark. A watermark row that names an unknown turn is treated as
    /// "commit everything" (safe direction — worst case a duplicate dedups).
    pub async fn pending_turns(
        &self,
        conversation_id: &str,
        turns: &[SessionTurn],
    ) -> Vec<SessionTurn> {
        self.pending_turns_inner(conversation_id, turns).await
    }

    async fn pending_turns_inner(
        &self,
        conversation_id: &str,
        turns: &[SessionTurn],
    ) -> Vec<SessionTurn> {
        let scoped: Vec<SessionTurn> = turns
            .iter()
            .filter(|turn| turn.conversation_id == conversation_id)
            .cloned()
            .collect();
        let Some(watermark) = self.watermark(conversation_id).await.ok().flatten() else {
            return scoped;
        };
        if watermark.last_turn_id.is_empty() {
            return scoped;
        }
        let position = scoped
            .iter()
            .position(|turn| turn.turn.turn_id == watermark.last_turn_id);
        match position {
            Some(index) => scoped[index + 1..].to_vec(),
            None => scoped,
        }
    }
}

/// Chunk metadata keys a session chunk carries beyond `Chunk`'s own fields —
/// used by the query side to filter by conversation.
pub const META_CONVERSATION_ID: &str = "conversation_id";
pub const META_ROLE: &str = "role";
pub const META_TIMESTAMP: &str = "timestamp";

/// The metadata block merged into a session chunk's vector row.
pub fn session_chunk_meta(turn: &SessionTurn) -> Value {
    json!({
        META_CONVERSATION_ID: turn.conversation_id,
        META_ROLE: turn.turn.role,
        META_TIMESTAMP: turn.turn.timestamp,
    })
}
