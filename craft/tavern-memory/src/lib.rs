//! The tavern craft: canon and conflict detection for roleplay sessions.
//!
//! A tavern session is a story too. The session has a **story time** (the
//! order events happened inside the fiction, which is not the order the
//! transcript was written), and the world has a **canon** — the character
//! sheets and world rules that the session must not contradict.
//!
//! Two layers of fact, with the kinds the engine already defines:
//!
//! | kind | what | example |
//! |---|---|---|
//! | `semantic` | canon: character sheets, world rules | "She never smokes." |
//! | `episodic` | what happened in a session | "She lit a cigarette at dawn." |
//!
//! A conflict is an episodic fact that contradicts a semantic one. The
//! detection is a comparison against the canon, not a re-derivation: the
//! craft decides *that* two statements collide, and the judge decides which
//! one wins.
//!
//! The scope follows the design doc (03 §1): **one character or world per
//! workspace**, the session carries `session_id`, and `task_id` stays empty.
//! A project-scoped recall with `session_id = None` therefore sees the whole
//! character's canon, which is what a world-book rendering wants.

use std::collections::BTreeMap;

use mnemo2u::core::types::{Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
use narrative_core::hash::content_hash;
use narrative_core::storytime::{Layer, StoryUnit, Tick, TICK_SCALE};

pub mod config;
pub mod conflict;
pub mod store;

pub use config::Config;
pub use conflict::{detect, disagrees, Subject};
pub use store::{
    conflicts_of, default_store_root, event_id, plan, Coder, MemoryBackend, WritePlan, WriteReport,
};

/// The kind of a tavern fact: canon or event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    /// Canon: a character sheet or a world rule. Never contradicted by an
    /// event — an event that contradicts canon is a conflict to resolve, not
    /// a correction to apply.
    Canon,
    /// An event that happened inside a session.
    Event,
}

impl FactKind {
    /// The `kind` string written into the fact store.
    ///
    /// These are the engine's existing kind words, so a tavern fact reads the
    /// same as any other fact: `semantic` for canon, `episodic` for events.
    pub fn as_str(&self) -> &'static str {
        match self {
            FactKind::Canon => "semantic",
            FactKind::Event => "episodic",
        }
    }
}

/// The scope of one tavern session.
///
/// One character or world is one workspace; a conversation is one session.
/// `task_id` stays empty: a tavern has no topics to partition by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TavernScope {
    /// The character or world this session belongs to.
    pub character: String,
    /// The conversation.
    pub session: String,
}

impl TavernScope {
    /// Build the scope.
    pub fn new(character: impl Into<String>, session: impl Into<String>) -> Self {
        Self {
            character: character.into(),
            session: session.into(),
        }
    }

    /// The engine scope for this session.
    pub fn to_scope_key(&self) -> ScopeKey {
        ScopeKey {
            workspace_id: self.character.clone(),
            session_id: Some(self.session.clone()),
            task_id: None,
        }
    }

    /// The engine scope for the whole character: no session constraint, so a
    /// recall sees every session's canon and every session's events.
    pub fn to_character_scope(&self) -> ScopeKey {
        ScopeKey {
            workspace_id: self.character.clone(),
            session_id: None,
            task_id: None,
        }
    }
}

/// A canon entry: one statement about the character or the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonEntry {
    /// The slot key (a position, not a value).
    pub key: String,
    /// The statement.
    pub text: String,
    /// Which layer of the world it belongs to.
    pub layer: Layer,
    /// Where it came from.
    pub source_path: String,
    /// The line it came from (1-based).
    pub source_line: usize,
}

impl CanonEntry {
    /// The content hash of the statement.
    ///
    /// The hash decides whether a rescan sees a change; it never enters the
    /// fact id, because a moving id would break the supersede chain.
    pub fn content_hash(&self) -> String {
        content_hash(&self.text)
    }
}

/// An event from a session: something that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    /// The slot key.
    pub key: String,
    /// What happened.
    pub text: String,
    /// Story time of the event.
    pub tick: Tick,
    /// Where it came from.
    pub source_path: String,
    /// The line it came from (1-based).
    pub source_line: usize,
}

/// A conflict between an event and a canon entry.
///
/// The craft reports the collision and leaves the decision to the judge: which
/// statement survives is a judgement about the fiction, not a rule this crate
/// can apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The canon entry's slot key.
    pub canon_key: String,
    /// The canon statement.
    pub canon_text: String,
    /// The event's slot key.
    pub event_key: String,
    /// The event statement.
    pub event_text: String,
    /// The story time of the event.
    pub tick: Tick,
}

impl Conflict {
    /// A one-line rendering for a report.
    pub fn line(&self) -> String {
        format!(
            "- canon `{}` vs event `{}` (story {})\n  canon: {}\n  event: {}",
            self.canon_key,
            self.event_key,
            narrative_core::storytime::format_tick_label(self.tick),
            self.canon_text,
            self.event_text
        )
    }
}

/// The tavern's memory: its canon and its events.
#[derive(Debug, Clone, Default)]
pub struct TavernMemory {
    /// The canon entries, keyed by slot.
    pub canon: BTreeMap<String, CanonEntry>,
    /// The events, keyed by slot.
    pub events: BTreeMap<String, SessionEvent>,
    /// The story units the events fall into.
    pub units: Vec<StoryUnit>,
}

impl TavernMemory {
    /// The canon entries of one layer.
    pub fn canon_of(&self, layer: Layer) -> impl Iterator<Item = &CanonEntry> {
        self.canon
            .values()
            .filter(move |entry| entry.layer == layer)
    }

    /// The events in story order.
    pub fn events_in_story_order(&self) -> Vec<&SessionEvent> {
        let mut events: Vec<&SessionEvent> = self.events.values().collect();
        events.sort_by_key(|event| event.tick);
        events
    }
}

/// The story-time coordinate of an event that carries no explicit marker.
///
/// The session's story order is the order of the transcript: the first event
/// is at one beat, the second at two. A tavern transcript is written in order
/// — it is the fiction itself, not a description of it — so the write order
/// *is* the story order, unless a marker says otherwise.
#[must_use]
pub fn tick_of_transcript_position(position: usize) -> Tick {
    (position as i64 + 1) * TICK_SCALE
}

/// The story-time coordinate of a story unit.
#[must_use]
pub fn tick_of_unit(unit: &StoryUnit) -> Tick {
    unit.tick
}

/// Turn a canon entry into a fact.
///
/// The fact's id is the slot key, so a rescan that changes the text
/// supersedes the old row instead of writing a second one.
#[must_use]
pub fn canon_fact(scope: &ScopeKey, entry: &CanonEntry, session_id: &str, created_at: i64) -> Fact {
    Fact {
        id: FactId(entry.key.clone()),
        scope: scope.clone(),
        kind: FactKind::Canon.as_str().to_string(),
        text: entry.text.clone(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: session_id.to_string(),
            branch_id: "canon".to_string(),
            epoch: 0,
            event_id: format!("{}#{}", entry.key, entry.content_hash()),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at,
        superseded_by: None,
        revision: 0,
    }
}

/// Turn a session event into a fact.
#[must_use]
pub fn event_fact(scope: &ScopeKey, event: &SessionEvent, created_at: i64) -> Fact {
    Fact {
        id: FactId(event.key.clone()),
        scope: scope.clone(),
        kind: FactKind::Event.as_str().to_string(),
        text: event.text.clone(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id.clone(),
            session_id: scope.session_id.clone().unwrap_or_default(),
            branch_id: "event".to_string(),
            epoch: 0,
            event_id: format!("{}#{}", event.key, content_hash(&event.text)),
        }],
        trust_state: TrustState::Verified,
        valid_from: Some(event.tick),
        valid_to: None,
        created_at,
        superseded_by: None,
        revision: 0,
    }
}

/// The edge kind that links an event to the canon it touches.
///
/// `Corrects` is the existing kind for "this statement revises that one", and
/// the tavern case is exactly that: the event says something the canon does
/// not allow, and the judge decides whether the canon is corrected or the
/// event was a mistake. The engine's kind vocabulary is shared by every
/// domain, so the tavern craft uses what is there rather than widening it.
#[must_use]
pub fn conflict_edge_kind() -> RelKind {
    RelKind::Corrects
}
