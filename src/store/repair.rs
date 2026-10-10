//! Repair queue — derived-index writes that failed are recorded in the truth
//! layer and replayed by the next flush (R1 commit protocol, T11).
//!
//! Rationale: turso is the only source of truth and commits *first*; a failed
//! derived write (graph/vector/report) must not lose the ingest and must not
//! need a re-extraction. The queue stores enough to replay: the extraction
//! records themselves, so the retry re-runs the same merge against the (now
//! committed) truth instead of caching a result.
//!
//! The queue lives in the truth layer, so it survives restarts and shares the
//! "replay everything from turso" invariant.

use std::sync::Arc;

use serde_json::Value as Json;

use crate::core::traits::{KvStore, Result};

/// KV namespace for the queue rows (separate from the reference stores).
pub const REPAIR_SCOPE: &str = "repair_queue";

/// Which replay routine a queued item needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairTarget {
    /// Re-run `merge_nodes_then_upsert` for one entity.
    GraphNode,
    /// Re-run `merge_edges_then_upsert` for one relation pair.
    GraphEdge,
    /// Re-upsert the chunk vector rows (naive RAG path).
    ChunkVector,
    /// Re-upsert the entity vector rows for one entity.
    EntityVector,
    /// Re-write the community report rows.
    CommunityReport,
    /// Re-upsert one relation vector row (`relationships_vdb`).
    RelationVector,
    /// Re-write one entity→chunk tracking row.
    EntityChunk,
    /// Re-write one relation→chunk tracking row.
    RelationChunk,
}

impl RepairTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::GraphNode => "graph_node",
            Self::GraphEdge => "graph_edge",
            Self::ChunkVector => "chunk_vector",
            Self::EntityVector => "entity_vector",
            Self::CommunityReport => "community_report",
            Self::RelationVector => "relation_vector",
            Self::EntityChunk => "entity_chunk",
            Self::RelationChunk => "relation_chunk",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "graph_node" => Some(Self::GraphNode),
            "graph_edge" => Some(Self::GraphEdge),
            "chunk_vector" => Some(Self::ChunkVector),
            "entity_vector" => Some(Self::EntityVector),
            "community_report" => Some(Self::CommunityReport),
            _ => None,
        }
    }
}

/// One queued write: stable id (re-recording replaces), replay target and the
/// payload the replay routine consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairItem {
    pub id: String,
    pub target: RepairTarget,
    pub payload: Json,
}

pub struct RepairQueue {
    kv: Arc<dyn KvStore>,
}

impl RepairQueue {
    pub fn new(kv: Arc<dyn KvStore>) -> Self {
        Self { kv }
    }

    pub fn item_id(target: RepairTarget, name: &str) -> String {
        format!("{}:{name}", target.as_str())
    }

    /// Record (or replace) a failed write. Re-recording the same target+name is
    /// idempotent, so a run that fails twice does not grow the queue.
    pub async fn record(&self, target: RepairTarget, name: &str, payload: Json) -> Result<()> {
        self.kv
            .upsert(vec![(
                Self::item_id(target, name),
                serde_json::json!({ "target": target.as_str(), "payload": payload }),
            )])
            .await
    }

    pub async fn pending(&self) -> Result<Vec<RepairItem>> {
        let mut items = Vec::new();
        for key in self.kv.all_keys().await? {
            let Some(row) = self.kv.get_by_id(&key).await? else {
                continue;
            };
            let Some(target) = row
                .get("target")
                .and_then(Json::as_str)
                .and_then(RepairTarget::parse)
            else {
                continue;
            };
            let payload = row.get("payload").cloned().unwrap_or(Json::Null);
            items.push(RepairItem {
                id: key,
                target,
                payload,
            });
        }
        items.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(items)
    }

    /// Drop replayed items; `ids` must come from [`RepairQueue::pending`].
    pub async fn done(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.kv.remove(ids).await
    }
}
