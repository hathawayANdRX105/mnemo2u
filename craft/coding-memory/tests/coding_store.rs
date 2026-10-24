//! The coding craft's store wiring: scope arithmetic, ids, and idempotency.
//!
//! These tests touch no embedding model. Opening the vector store needs
//! fastembed, which needs a model, so the plan — the function that decides
//! which scopes, ids and edges a write produces — is where the craft's
//! behaviour is pinned. The I/O path mirrors the novel craft's, which the
//! review tool exercises in production.

use coding_memory::store::{content_hash, event_id, hash_of_event_id, plan, WritePlan};
use coding_memory::{record_use, Episode, MemoryKind, Procedure, Rule, SessionRecord, TaskScope};
use mnemo2u::core::types::{RelKind, ScopeKey};

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

fn procedure(key: &str, rule_key: Option<&str>) -> Procedure {
    Procedure {
        key: key.to_string(),
        title: "Cut a release".to_string(),
        body: "1. run the tests\n2. tag the commit\nexpected: a green build".to_string(),
        inputs: vec!["a clean tree".to_string()],
        expected: "a green build".to_string(),
        derived_from: rule_key.map(str::to_string),
        used_count: 0,
        last_outcome: None,
    }
}

fn facts_of<'a>(plan: &'a WritePlan, id: &str) -> &'a mnemo2u::core::types::Fact {
    plan.facts
        .iter()
        .find(|fact| fact.id.0 == id)
        .unwrap_or_else(|| panic!("no fact with id {id}"))
}

#[test]
fn an_episode_keeps_its_session_id() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let plan = plan(
        &SessionRecord {
            episodes: vec![episode("episode:1", "The build broke.", Some("turn-7"))],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let fact = facts_of(&plan, "turn:turn-7");
    // Session-bound: a pit hit in one run is not a fact about the project.
    assert_eq!(fact.scope.session_id.as_deref(), Some("exec-1"));
    assert_eq!(fact.scope.task_id, None);
    assert_eq!(fact.kind, "episodic");
}

#[test]
fn a_rule_drops_the_session_but_keeps_the_topic() {
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule("rule:lockfile", "Building needs cargo fetch.", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let fact = facts_of(&plan, "rule:lockfile");
    // Project knowledge: the session id goes, or a project-wide recall could
    // never read back what one run learned.
    assert_eq!(fact.scope.session_id, None);
    // The topic stays, so a topic-scoped query finds it too.
    assert_eq!(fact.scope.task_id.as_deref(), Some("topic:release"));
    assert_eq!(fact.kind, "semantic");
}

#[test]
fn a_procedure_is_procedural_and_session_free() {
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release");
    let plan = plan(
        &SessionRecord {
            procedures: vec![procedure("procedure:release", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let fact = facts_of(&plan, "procedure:release");
    assert_eq!(fact.kind, "procedural");
    assert_eq!(fact.scope.session_id, None);
    assert_eq!(fact.scope.task_id.as_deref(), Some("topic:release"));
}

#[test]
fn a_rule_records_the_episode_it_came_from() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule(
                "rule:lockfile",
                "Building needs cargo fetch.",
                Some("turn-7"),
            )],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let fact = facts_of(&plan, "rule:lockfile");
    // The provenance rides in the source reference.
    assert_eq!(fact.source_refs[0].branch_id, "turn-7");
}

#[test]
fn the_plan_carries_a_rule_depends_on_edge_and_a_procedure_edge() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule(
                "rule:lockfile",
                "Building needs cargo fetch.",
                Some("turn-7"),
            )],
            procedures: vec![procedure("procedure:release", Some("rule:lockfile"))],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let depends = plan
        .edges
        .iter()
        .find(|edge| edge.src.0 == "rule:lockfile")
        .expect("the rule depends on the episode that established it");
    assert_eq!(depends.dst.0, "turn-7");
    assert_eq!(depends.rel, RelKind::DependsOn);

    let from_rule = plan
        .edges
        .iter()
        .find(|edge| edge.src.0 == "procedure:release")
        .expect("the procedure comes from the rule");
    assert_eq!(from_rule.dst.0, "rule:lockfile");
    assert_eq!(from_rule.rel, RelKind::Corrects);
}

#[test]
fn derivation_edges_cross_sessions_by_construction() {
    // An Edge carries no scope of its own — it is written under whatever scope
    // the writer passes to upsert_edges. What this test pins is that a rule's
    // provenance edge exists at all, and links the two facts it should.
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule(
                "rule:lockfile",
                "Building needs cargo fetch.",
                Some("turn-7"),
            )],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let edge = plan.edges.first().expect("one edge");
    assert_eq!(edge.src.0, "rule:lockfile");
    // The endpoint is the episode's fact id, which a turn id maps to directly.
    assert_eq!(edge.dst.0, "turn:turn-7");
}

#[test]
fn a_project_wide_recall_sees_every_session_rules() {
    // What the scope choice is for: the write happens in one session, the
    // read asks about the project.
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release");
    let plan = plan(
        &SessionRecord {
            episodes: vec![episode("episode:1", "The build broke.", Some("turn-7"))],
            rules: vec![rule("rule:lockfile", "Building needs cargo fetch.", None)],
            procedures: vec![procedure("procedure:release", None)],
        },
        &scope,
        0,
    );
    // A project-wide recall: no session, no topic.
    let whole_project = ScopeKey {
        workspace_id: "mnemo2u".to_string(),
        session_id: None,
        task_id: None,
    };
    let rule = facts_of(&plan, "rule:lockfile");
    assert!(
        whole_project.admits(&rule.scope),
        "a rule is project knowledge"
    );
    let procedure = facts_of(&plan, "procedure:release");
    assert!(
        whole_project.admits(&procedure.scope),
        "a procedure is project knowledge"
    );

    // A session recall does not see them: they were never about that run.
    let one_session = ScopeKey {
        workspace_id: "mnemo2u".to_string(),
        session_id: Some("other-exec".to_string()),
        task_id: None,
    };
    assert!(
        !one_session.admits(&rule.scope),
        "another session must not adopt this rule"
    );
    // The episode belongs to its own session, so a different one cannot see
    // it either — the provenance an edge points at is fetchable from within
    // the session that produced it.
    let episode = facts_of(&plan, "turn:turn-7");
    assert!(!one_session.admits(&episode.scope));
    let its_own_session = ScopeKey {
        workspace_id: "mnemo2u".to_string(),
        session_id: Some("exec-1".to_string()),
        task_id: None,
    };
    assert!(its_own_session.admits(&episode.scope));
}

#[test]
fn a_topic_recall_sees_the_topic_rules_and_procedures() {
    let scope = TaskScope::in_topic("mnemo2u", "exec-1", "topic:release");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule("rule:lockfile", "Building needs cargo fetch.", None)],
            procedures: vec![procedure("procedure:release", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let the_topic = ScopeKey {
        workspace_id: "mnemo2u".to_string(),
        session_id: None,
        task_id: Some("topic:release".to_string()),
    };
    assert!(the_topic.admits(&facts_of(&plan, "rule:lockfile").scope));
    assert!(the_topic.admits(&facts_of(&plan, "procedure:release").scope));
    // A different topic does not inherit another topic's knowledge.
    let other_topic = ScopeKey {
        workspace_id: "mnemo2u".to_string(),
        session_id: None,
        task_id: Some("topic:docs".to_string()),
    };
    assert!(!other_topic.admits(&facts_of(&plan, "rule:lockfile").scope));
}

#[test]
fn another_projects_workspace_is_invisible() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule("rule:lockfile", "Building needs cargo fetch.", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let elsewhere = ScopeKey {
        workspace_id: "some-other-repo".to_string(),
        session_id: None,
        task_id: None,
    };
    assert!(!elsewhere.admits(&facts_of(&plan, "rule:lockfile").scope));
}

#[test]
fn the_fact_id_is_the_slot_key_never_the_text() {
    // An id that moves orphans the row supersede would look for.
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let first = plan(
        &SessionRecord {
            rules: vec![rule("rule:lockfile", "Building needs cargo fetch.", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let after_edit = plan(
        &SessionRecord {
            rules: vec![rule(
                "rule:lockfile",
                "Building needs cargo fetch and a lock.",
                None,
            )],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    assert_eq!(facts_of(&first, "rule:lockfile").id.0, "rule:lockfile");
    assert_eq!(
        facts_of(&after_edit, "rule:lockfile").id.0,
        "rule:lockfile",
        "the id survives the edit, so supersede can find the old row"
    );
}

#[test]
fn the_content_hash_travels_in_the_event_id_and_decides_idempotency() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let text = "Building needs cargo fetch.";
    let plan = plan(
        &SessionRecord {
            rules: vec![rule("rule:lockfile", text, None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let event_id = &facts_of(&plan, "rule:lockfile").source_refs[0].event_id;
    assert!(event_id.starts_with("rule:lockfile#"));
    // The same text hashes to the same value, so a rescan reads as unchanged.
    assert_eq!(
        hash_of_event_id(event_id),
        Some(content_hash(text).as_str())
    );
    // Different text hashes differently, so the same slot reads as changed.
    assert_ne!(
        content_hash(text),
        content_hash("Building needs something else.")
    );
}

#[test]
fn an_episode_without_a_turn_id_falls_back_to_the_slot_key() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let plan = plan(
        &SessionRecord {
            episodes: vec![episode("episode:1", "Something happened.", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    assert_eq!(facts_of(&plan, "episode:1").id.0, "episode:1");
}

#[test]
fn the_baseline_reaches_the_facts_created_at() {
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let plan = plan(
        &SessionRecord {
            rules: vec![rule("rule:lockfile", "Building needs cargo fetch.", None)],
            ..SessionRecord::default()
        },
        &scope,
        1_700_000_000_000,
    );
    assert_eq!(
        facts_of(&plan, "rule:lockfile").created_at,
        1_700_000_000_000
    );
}

#[test]
fn the_event_id_helper_round_trips() {
    let id = event_id("slot:key", "the text");
    assert_eq!(id, "slot:key#".to_string() + &content_hash("the text"));
    assert_eq!(hash_of_event_id("slot:key"), None, "no # means no hash");
}

#[test]
fn using_a_procedure_records_the_count_and_the_outcome() {
    // The track record is the caller's to keep; the store writes the
    // definition. This is the helper the harness maintains its copy with.
    let procedure = record_use(procedure("procedure:release", None), "green");
    assert_eq!(procedure.used_count, 1);
    assert_eq!(procedure.last_outcome.as_deref(), Some("green"));
    // The definition is untouched by a use, so a rewrite stays idempotent.
    assert_eq!(
        procedure.body,
        "1. run the tests\n2. tag the commit\nexpected: a green build"
    );
}

#[test]
fn a_procedures_definition_does_not_change_when_its_track_record_does() {
    // The whole point of keeping the counter out of the stored text: writing
    // a procedure after a use must not look like a correction.
    let scope = TaskScope::new("mnemo2u", "exec-1");
    let before = plan(
        &SessionRecord {
            procedures: vec![procedure("procedure:release", None)],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    let mut used = procedure("procedure:release", None);
    used.used_count = 12;
    used.last_outcome = Some("red".to_string());
    let after = plan(
        &SessionRecord {
            procedures: vec![used],
            ..SessionRecord::default()
        },
        &scope,
        0,
    );
    assert_eq!(
        hash_of_event_id(&facts_of(&before, "procedure:release").source_refs[0].event_id),
        hash_of_event_id(&facts_of(&after, "procedure:release").source_refs[0].event_id),
        "a use must not change what the store would write"
    );
}

#[test]
fn the_three_kinds_are_the_engines_vocabulary() {
    assert_eq!(MemoryKind::Episode.as_str(), "episodic");
    assert_eq!(MemoryKind::Rule.as_str(), "semantic");
    assert_eq!(MemoryKind::Procedure.as_str(), "procedural");
}
