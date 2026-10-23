//! The tavern craft: scopes, canon kinds, and conflict detection.
//!
//! These tests touch no embedding model — the craft's logic here is scope
//! arithmetic and statement comparison, both pure.

use narrative_core::storytime::{Layer, TICK_SCALE};
use tavern_memory::{
    canon_fact, conflict_edge_kind, detect, disagrees, event_fact, tick_of_transcript_position,
    CanonEntry, FactKind, SessionEvent, Subject, TavernScope,
};

fn canon(key: &str, text: &str) -> CanonEntry {
    CanonEntry {
        key: key.to_string(),
        text: text.to_string(),
        layer: Layer::Setting,
        source_path: "character.md".to_string(),
        source_line: 1,
    }
}

fn event(key: &str, text: &str, position: usize) -> SessionEvent {
    SessionEvent {
        key: key.to_string(),
        text: text.to_string(),
        tick: tick_of_transcript_position(position),
        source_path: "transcript.md".to_string(),
        source_line: position + 1,
    }
}

#[test]
fn one_character_is_one_workspace_and_a_session_is_a_session() {
    let scope = TavernScope::new("lilian", "session-1");
    let key = scope.to_scope_key();
    assert_eq!(key.workspace_id, "lilian");
    assert_eq!(key.session_id.as_deref(), Some("session-1"));
    // A tavern has no topics to partition by.
    assert_eq!(key.task_id, None);
}

#[test]
fn a_character_scope_sees_every_session() {
    let scope = TavernScope::new("lilian", "session-1");
    let character = scope.to_character_scope();
    assert_eq!(character.workspace_id, "lilian");
    // No session constraint: a recall sees the whole character's canon.
    assert_eq!(character.session_id, None);
}

#[test]
fn canon_is_semantic_and_events_are_episodic() {
    assert_eq!(FactKind::Canon.as_str(), "semantic");
    assert_eq!(FactKind::Event.as_str(), "episodic");
}

#[test]
fn a_canon_fact_keeps_its_slot_key_as_the_id() {
    let scope = TavernScope::new("lilian", "session-1").to_scope_key();
    let entry = canon("canon:smoking", "She never smokes.");
    let fact = canon_fact(&scope, &entry, "session-1", 0);
    assert_eq!(fact.id.0, "canon:smoking");
    assert_eq!(fact.kind, "semantic");
    // The content hash rides in the event id, never in the fact id: a moving
    // id would break the supersede chain.
    assert!(fact.source_refs[0].event_id.starts_with("canon:smoking#"));
    assert_eq!(
        fact.source_refs[0].event_id.split('#').nth(1),
        Some(entry.content_hash().as_str())
    );
}

#[test]
fn an_event_fact_carries_its_story_time() {
    let scope = TavernScope::new("lilian", "session-1").to_scope_key();
    let fact = event_fact(&scope, &event("event:1", "She lit a cigarette.", 0), 0);
    assert_eq!(fact.kind, "episodic");
    assert_eq!(fact.valid_from, Some(TICK_SCALE));
    assert_eq!(fact.valid_to, None);
}

#[test]
fn the_transcript_order_is_the_story_order() {
    // The first event is at one beat, the second at two.
    assert_eq!(tick_of_transcript_position(0), TICK_SCALE);
    assert_eq!(tick_of_transcript_position(1), 2 * TICK_SCALE);
}

#[test]
fn a_negated_statement_contradicts_a_plain_one() {
    let canon_subject = Subject::of("She never smokes.");
    let event_subject = Subject::of("She lit a cigarette.");
    assert!(canon_subject.same_subject(&event_subject));
    assert!(disagrees(&canon_subject, &event_subject));
}

#[test]
fn two_plain_statements_about_the_same_subject_do_not_contradict() {
    let left = Subject::of("She drinks tea.");
    let right = Subject::of("She drinks tea in the morning.");
    assert!(left.same_subject(&right));
    assert!(!disagrees(&left, &right));
}

#[test]
fn different_values_for_the_same_attribute_contradict() {
    let left = Subject::of("Her eyes are blue.");
    let right = Subject::of("Her eyes are green.");
    assert!(left.same_subject(&right));
    assert!(disagrees(&left, &right));
}

#[test]
fn statements_about_different_subjects_never_contradict() {
    let left = Subject::of("She never smokes.");
    let right = Subject::of("He always smokes.");
    assert!(!left.same_subject(&right));
    assert!(!disagrees(&left, &right));
}

#[test]
fn two_statements_about_different_subjects_do_not_contradict_even_with_the_same_negation() {
    // The subject check comes before the negation check, so this is not a
    // contradiction.
    let left = Subject::of("She never smokes.");
    let right = Subject::of("He never smokes.");
    assert!(!left.same_subject(&right));
    assert!(!disagrees(&left, &right));
}

#[test]
fn a_negation_against_a_plain_statement_about_the_same_subject_contradicts() {
    let left = Subject::of("She never smokes.");
    let right = Subject::of("She smokes.");
    assert!(left.same_subject(&right));
    assert!(disagrees(&left, &right));
}

#[test]
fn detection_reports_the_collision_with_both_statements() {
    let canon = canon("canon:smoking", "She never smokes.");
    let events = [event("event:1", "She lit a cigarette.", 0)];
    let conflicts = detect([&canon], events.iter());
    assert_eq!(conflicts.len(), 1);
    let conflict = &conflicts[0];
    assert_eq!(conflict.canon_key, "canon:smoking");
    assert_eq!(conflict.event_key, "event:1");
    assert_eq!(conflict.tick, TICK_SCALE);
    assert!(conflict.line().contains("She never smokes."));
}

#[test]
fn a_consistent_session_reports_nothing() {
    let canon = canon("canon:tea", "She drinks tea.");
    let events = [event("event:1", "She drank tea at dawn.", 0)];
    assert!(detect([&canon], events.iter()).is_empty());
}

#[test]
fn a_conflict_uses_the_engines_existing_edge_kind() {
    // The kind vocabulary is shared by every domain; the tavern craft uses
    // what is there rather than widening it.
    assert_eq!(
        conflict_edge_kind(),
        mnemo2u::core::types::RelKind::Corrects
    );
}
