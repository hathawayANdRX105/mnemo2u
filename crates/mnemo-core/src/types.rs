//! Core data types for mnemo2u.

use serde::{Deserialize, Serialize};

/// Stable identifier of a fact entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactId(pub String);

/// Reference back to the original evidence in the harness SessionDb.
/// The encoding is host-generated; possession of the ref is not authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub workspace_id: String,
    pub session_id: String,
    pub branch_id: String,
    pub epoch: u64,
    pub event_id: String,
}

/// Scope key: memory never crosses user/project/session boundaries.
/// Part of the primary key and of the embedding cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeKey {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
}

/// Trust state of a memory (trust-state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustState {
    Candidate,
    Verified,
    Rejected,
    Stale,
}

/// A derived fact. Never the source of truth: `source_refs` always points at
/// retained transcript evidence, and bi-temporal columns separate "when it was
/// true" from "when we learned it".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: FactId,
    pub scope: ScopeKey,
    /// semantic | episodic | procedural
    pub kind: String,
    pub text: String,
    pub source_refs: Vec<SourceRef>,
    pub trust_state: TrustState,
    /// When the fact held in the world (application time).
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
    /// When the system learned it (system time).
    pub created_at: i64,
    pub superseded_by: Option<FactId>,
    pub revision: u64,
}

/// Relation carried on graph edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelKind {
    Supersedes,
    DependsOn,
    AboutTask,
    MentionsFile,
    CausedBy,
    Corrects,
    Related,
}

/// A graph edge: relations live here, fact bodies live in facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub src: FactId,
    pub dst: FactId,
    pub rel: RelKind,
    pub weight: f64,
    pub source_event_id: String,
}
