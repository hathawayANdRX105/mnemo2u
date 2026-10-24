//! The coding craft: task memory for a coding harness.
//!
//! A coding session produces three kinds of thing worth remembering, and they
//! have different lifetimes:
//!
//! | kind | what | lifetime |
//! |---|---|---|
//! | `episodic` | what happened: the pit hit, the decision taken, the fix applied | one session |
//! | `semantic` | a rule: "in this repo, running X requires Y" | the project |
//! | `procedural` | a procedure: a repeatable sequence of steps for a task | the project |
//!
//! The engine already knows how to derive facts from closed turns and how to
//! judge their kind. What the coding craft adds is the **shape of the work**:
//! the scope of a task, the way a rule names the experiment it came from, and
//! the way a procedure records that it was used and what came of it.
//!
//! The scope follows the design doc (03 §1): a **project** (a repository) is
//! the workspace, an execution session is the `session_id`, and a task family
//! is the `task_id` (`task_id = "topic:release"` and the like). A project-wide
//! recall leaves `session_id` empty, which is how a procedure set is read back
//! across every session that produced it.
//!
//! The kinds are the engine's existing vocabulary — `semantic`, `episodic`,
//! `procedural`. The craft does not widen it.

pub mod config;
pub mod store;

pub use config::Config;
pub use store::{
    content_hash, default_store_root, event_id, hash_of_event_id, plan, Coder, MemoryBackend,
    SessionRecord, WritePlan, WriteReport,
};

use mnemo2u::core::types::{Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};

/// The scope of one coding task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskScope {
    /// The repository (the project).
    pub project: String,
    /// The execution session.
    pub session: String,
    /// The task family, e.g. `topic:release`. Empty means the whole project.
    pub topic: Option<String>,
}

impl TaskScope {
    /// Build a scope.
    pub fn new(project: impl Into<String>, session: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            session: session.into(),
            topic: None,
        }
    }

    /// Build a scope inside a task family.
    pub fn in_topic(
        project: impl Into<String>,
        session: impl Into<String>,
        topic: impl Into<String>,
    ) -> Self {
        Self {
            project: project.into(),
            session: session.into(),
            topic: Some(topic.into()),
        }
    }

    /// The engine scope for this task.
    pub fn to_scope_key(&self) -> ScopeKey {
        ScopeKey {
            workspace_id: self.project.clone(),
            session_id: Some(self.session.clone()),
            task_id: self.topic.clone(),
        }
    }

    /// The engine scope for the whole project: no session constraint, so a
    /// recall sees everything every session produced.
    pub fn to_project_scope(&self) -> ScopeKey {
        ScopeKey {
            workspace_id: self.project.clone(),
            session_id: None,
            task_id: self.topic.clone(),
        }
    }
}

/// The three kinds of thing a coding session remembers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// What happened: a pit, a decision, a fix. Session-bound.
    Episode,
    /// A rule: a constraint that holds for the project.
    Rule,
    /// A procedure: a repeatable sequence of steps.
    Procedure,
}

impl MemoryKind {
    /// The `kind` string written into the fact store.
    ///
    /// The engine's own vocabulary: `episodic`, `semantic`, `procedural`. A
    /// rule is a semantic fact — it is knowledge about the project, not an
    /// event — and a procedure is the third kind the engine already defines.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Episode => "episodic",
            MemoryKind::Rule => "semantic",
            MemoryKind::Procedure => "procedural",
        }
    }
}

/// An episode: something that happened in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    /// The slot key.
    pub key: String,
    /// What happened.
    pub text: String,
    /// The turn it came from, if it came from one.
    pub turn_id: Option<String>,
}

impl Episode {
    /// The fact id an episode gets.
    ///
    /// A turn id is stable and unique inside a conversation, so it is the
    /// natural key: the same turn replayed produces the same fact rather
    /// than a second one.
    #[must_use]
    pub fn fact_id(&self) -> String {
        match &self.turn_id {
            Some(turn_id) => format!("turn:{turn_id}"),
            None => self.key.clone(),
        }
    }
}

/// A rule: a constraint that holds for the project.
///
/// A rule names the episode it came from, so the reasoning stays traceable:
/// "this repo requires X" points at the run that learned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The slot key.
    pub key: String,
    /// The rule's text.
    pub text: String,
    /// The episode this rule was generalised from.
    pub derived_from: Option<String>,
}

/// A procedure: a repeatable sequence of steps.
///
/// The body is the three-part text the engine's procedure contract asks for:
/// what the steps are, what they need, and what a good outcome looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Procedure {
    /// The slot key.
    pub key: String,
    /// The title.
    pub title: String,
    /// The steps, what they need, and what a good outcome looks like.
    pub body: String,
    /// The inputs the steps need.
    pub inputs: Vec<String>,
    /// The expected outcome.
    pub expected: String,
    /// The rule this procedure was made from, when it came from one.
    ///
    /// The link is what lets a recall answer "which rule produced these steps",
    /// which is the first question asked of a procedure that went wrong.
    pub derived_from: Option<String>,
    /// How many times the procedure was used.
    ///
    /// The caller's to keep, not the store's: the written fact carries the
    /// procedure's *definition*, and re-writing it on every use would build a
    /// supersede chain out of a counter that is not part of the procedure. A
    /// harness tracks the record in its own session store.
    pub used_count: u64,
    /// What the last use produced. Also the caller's to keep — see
    /// [`Procedure::used_count`].
    pub last_outcome: Option<String>,
}

/// The edge from a rule to the episode it came from.
///
/// The engine has `DependsOn` for "this depends on that", which is exactly the
/// relation: the rule depends on the run that established it.
#[must_use]
pub fn rule_depends_on_kind() -> RelKind {
    RelKind::DependsOn
}

/// The edge from a procedure back to the rule it came from.
///
/// The engine has no `Generalizes` edge yet (the design doc proposes one for
/// episode → rule → procedure). Rather than widen the shared vocabulary for
/// one craft, `Corrects` is the existing kind that carries "this revises
/// that", and a procedure does revise the rule it was made from. When the
/// engine grows `Generalizes`, this is the one line that changes.
#[must_use]
pub fn procedure_from_rule_kind() -> RelKind {
    RelKind::Corrects
}

/// Turn an episode into a fact.
#[must_use]
pub fn episode_fact(scope: &ScopeKey, episode: &Episode, created_at: i64) -> Fact {
    Fact {
        id: FactId(episode.fact_id()),
        scope: scope.clone(),
        kind: MemoryKind::Episode.as_str().to_string(),
        text: episode.text.clone(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: scope.session_id.clone().unwrap_or_default(),
            branch_id: episode.turn_id.clone().unwrap_or_default(),
            epoch: 0,
            event_id: episode.key.clone(),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at,
        superseded_by: None,
        revision: 0,
    }
}

/// Turn a rule into a fact.
#[must_use]
pub fn rule_fact(scope: &ScopeKey, rule: &Rule, created_at: i64) -> Fact {
    Fact {
        id: FactId(rule.key.clone()),
        scope: scope.clone(),
        kind: MemoryKind::Rule.as_str().to_string(),
        text: rule.text.clone(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: scope.session_id.clone().unwrap_or_default(),
            branch_id: rule.derived_from.clone().unwrap_or_default(),
            epoch: 0,
            event_id: rule.key.clone(),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at,
        superseded_by: None,
        revision: 0,
    }
}

/// Turn a procedure into a fact.
#[must_use]
pub fn procedure_fact(scope: &ScopeKey, procedure: &Procedure, created_at: i64) -> Fact {
    Fact {
        id: FactId(procedure.key.clone()),
        scope: scope.clone(),
        kind: MemoryKind::Procedure.as_str().to_string(),
        text: procedure.body.clone(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: scope.session_id.clone().unwrap_or_default(),
            branch_id: procedure.key.clone(),
            epoch: 0,
            event_id: procedure.key.clone(),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at,
        superseded_by: None,
        revision: 0,
    }
}

/// Record that a procedure was used and what came of it.
///
/// A procedure's usefulness is not in its text but in its track record, so
/// the count and the last outcome are what a recall weighs. This returns the
/// updated procedure; the caller decides whether to persist it.
#[must_use]
pub fn record_use(mut procedure: Procedure, outcome: &str) -> Procedure {
    procedure.used_count += 1;
    procedure.last_outcome = Some(outcome.to_string());
    procedure
}
