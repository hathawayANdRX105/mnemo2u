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

impl ScopeKey {
    /// Whether a stored item's scope falls inside a query scope: the
    /// workspace must match exactly, and a present session/task constraint
    /// must match exactly too — an absent constraint admits everything under
    /// the workspace. This is the shared pre-filter predicate every recall
    /// arm applies *inside* its own query (stage doc 03 contract 1), so a
    /// narrower caller never sees another scope's rows, nor even their ranks.
    pub fn admits(&self, item: &ScopeKey) -> bool {
        self.workspace_id == item.workspace_id
            && self
                .session_id
                .as_deref()
                .is_none_or(|session| item.session_id.as_deref() == Some(session))
            && self
                .task_id
                .as_deref()
                .is_none_or(|task| item.task_id.as_deref() == Some(task))
    }
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

impl Fact {
    /// Whether the row belongs to the current view: neither superseded nor
    /// closed by `valid_to`. One predicate shared by both fact stores and
    /// the recall paths (R5 contract v2) — readers must never re-derive it.
    pub fn is_current(&self) -> bool {
        self.superseded_by.is_none() && self.valid_to.is_none()
    }
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
