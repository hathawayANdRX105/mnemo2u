//! The coding craft: task scopes, the three memory kinds, and procedure use.
//!
//! These tests touch no embedding model — the craft's logic here is scope
//! arithmetic and fact shaping, both pure.

use coding_memory::{
    episode_fact, procedure_fact, procedure_from_rule_kind, record_use, rule_depends_on_kind,
    rule_fact, Episode, MemoryKind, Procedure, Rule, TaskScope,
};

fn episode(key: &str, text: &str, turn_id: Option<&str>) -> Episode {
    Episode {
        key: key.to_string(),
        text: text.to_string(),
        turn_id: turn_id.map(str::to_string),
    }
}

fn rule(key: &str, text: &str, derived_from: Option<&str>) -> Rule {
    Rule {
        key: key.to_string(),
        text: text.to_string(),
        derived_from: derived_from.map(str::to_string),
    }
}

fn procedure() -> Procedure {
    Procedure {
        key: "procedure:release".to_string(),
        title: "Cut a release".to_string(),
        body: "1. run the tests\n2. tag the commit\nexpected: a green build".to_string(),
        inputs: vec!["a clean tree".to_string()],
        expected: "a green build".to_string(),
        used_count: 0,
        last_outcome: None,
    }
}

#[test]
fn a_project_is_the_workspace_and_a_session_is_the_session() {
    let scope = TaskScope::new("mnemo2u", "exec-1").to_scope_key();
    assert_eq!(scope.workspace_id, "mnemo2u");
    assert_eq!(scope.session_id.as_deref(), Some("exec-1"));
    // No task family unless one is named.
    assert_eq!(scope.task_id, None);
}

#[test]
fn a_task_family_becomes_the_task_id() {
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release").to_scope_key();
    assert_eq!(scope.task_id.as_deref(), Some("topic:release"));
    // The session still applies.
    assert_eq!(scope.session_id.as_deref(), Some("exec-1"));
}

#[test]
fn a_project_scope_sees_every_session() {
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release").to_project_scope();
    assert_eq!(scope.workspace_id, "mnemo2u");
    // No session constraint: a recall sees what every session produced.
    assert_eq!(scope.session_id, None);
    // The task family stays: the topic is the unit of grouping.
    assert_eq!(scope.task_id.as_deref(), Some("topic:release"));
}

#[test]
fn the_three_kinds_use_the_engines_vocabulary() {
    assert_eq!(MemoryKind::Episode.as_str(), "episodic");
    assert_eq!(MemoryKind::Rule.as_str(), "semantic");
    assert_eq!(MemoryKind::Procedure.as_str(), "procedural");
}

#[test]
fn an_episode_is_keyed_by_its_turn() {
    // A turn id is stable inside a conversation, so replaying the same turn
    // produces the same fact rather than a second one.
    let with_turn = episode(
        "episode:1",
        "The build broke on the lockfile.",
        Some("turn-7"),
    );
    assert_eq!(with_turn.fact_id(), "turn:turn-7");
    // Without one, the slot key stands.
    let without_turn = episode("episode:1", "The build broke on the lockfile.", None);
    assert_eq!(without_turn.fact_id(), "episode:1");
}

#[test]
fn an_episode_fact_is_episodic_and_session_bound() {
    let scope = TaskScope::new("mnemo2u", "exec-1").to_scope_key();
    let fact = episode_fact(
        &scope,
        &episode("episode:1", "The build broke.", Some("turn-7")),
        0,
    );
    assert_eq!(fact.kind, "episodic");
    assert_eq!(fact.id.0, "turn:turn-7");
    assert_eq!(fact.scope.session_id.as_deref(), Some("exec-1"));
}

#[test]
fn a_rule_fact_records_the_episode_it_came_from() {
    let scope = TaskScope::new("mnemo2u", "exec-1").to_scope_key();
    let fact = rule_fact(
        &scope,
        &rule(
            "rule:lockfile",
            "Building this repo requires cargo fetch first.",
            Some("turn-7"),
        ),
        0,
    );
    assert_eq!(fact.kind, "semantic");
    // The provenance rides in the source reference, so the reasoning stays
    // traceable.
    assert_eq!(fact.source_refs[0].branch_id, "turn-7");
}

#[test]
fn a_procedure_fact_carries_the_body_and_the_procedural_kind() {
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release").to_scope_key();
    let fact = procedure_fact(&scope, &procedure(), 0);
    assert_eq!(fact.kind, "procedural");
    assert!(fact.text.contains("expected: a green build"));
    assert_eq!(fact.scope.task_id.as_deref(), Some("topic:release"));
}

#[test]
fn using_a_procedure_records_the_count_and_the_outcome() {
    let procedure = record_use(procedure(), "green");
    assert_eq!(procedure.used_count, 1);
    assert_eq!(procedure.last_outcome.as_deref(), Some("green"));

    let procedure = record_use(procedure, "red");
    assert_eq!(procedure.used_count, 2);
    assert_eq!(procedure.last_outcome.as_deref(), Some("red"));
}

#[test]
fn the_edges_use_the_engines_existing_kinds() {
    // The kind vocabulary is shared by every domain; the coding craft uses
    // what is there rather than widening it.
    assert_eq!(
        rule_depends_on_kind(),
        mnemo2u::core::types::RelKind::DependsOn
    );
    assert_eq!(
        procedure_from_rule_kind(),
        mnemo2u::core::types::RelKind::Corrects
    );
}
