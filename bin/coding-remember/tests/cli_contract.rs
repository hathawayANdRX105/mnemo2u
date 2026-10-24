//! The CLI's contract, as far as it can be tested without downloads.
//!
//! `open` builds a vector store, which needs the embedding model, so no test
//! here runs a command end to end. What *is* testable is everything the harness
//! depends on: the JSON a session arrives as, how a scope is resolved from the
//! flags, and how an answer is shaped. Those are pinned here.
//!
//! The store-facing behaviour (idempotent writes, scope arithmetic) is pinned by
//! `craft/coding-memory/tests/coding_store.rs`, which needs no model. This file
//! is about the process boundary.

use clap::Parser;
use coding_memory::store::SessionRecord;
use coding_memory::ScopeKey;
use coding_remember::cli::{scope_of_from_flags, Cli, Command};

/// The scope a set of flags resolves to. The unwrap is safe: the flags are
/// literals in these tests, and the only failure path is a malformed store path.
fn flags_to_scope(project: &str, session: Option<&str>, topic: Option<&str>) -> ScopeKey {
    scope_of_from_flags(project, session, topic).expect("the flags resolve")
}

#[test]
fn an_absent_session_item_and_an_empty_one_mean_the_same_scope() {
    // This is the difference between a project-wide recall and an empty
    // answer, so it is worth a test of its own.
    let absent = flags_to_scope("repo", None, None);
    let empty = flags_to_scope("repo", Some(""), None);
    assert_eq!(absent, empty);
    assert!(absent.session_id.is_none());
}

#[test]
fn a_named_session_scope_stays_named() {
    let scope = flags_to_scope("repo", Some("exec-7"), None);
    assert_eq!(scope.session_id.as_deref(), Some("exec-7"));
    // The workspace is still exact.
    assert_eq!(scope.workspace_id, "repo");
}

#[test]
fn a_topic_stays_on_the_scope() {
    let scope = flags_to_scope("repo", Some("exec-7"), Some("topic:release"));
    assert_eq!(scope.task_id.as_deref(), Some("topic:release"));
    let unscoped_topic = flags_to_scope("repo", None, Some("topic:release"));
    assert_eq!(unscoped_topic.task_id.as_deref(), Some("topic:release"));
    assert!(unscoped_topic.session_id.is_none());
}

#[test]
fn the_session_that_a_write_names_is_the_scope_it_reads_back_at() {
    // A write names one session through `remember --session`; the recall that
    // should see it names the same one through `recall --session`. The scope the
    // flags build for each has to be the same scope, or a write would be
    // invisible to the very read that asks for it.
    let written_at = flags_to_scope("repo", Some("exec-7"), None);
    let read_at = flags_to_scope("repo", Some("exec-7"), None);
    assert_eq!(
        written_at, read_at,
        "the write path and the read path spell one session identically"
    );
    // And a project-wide recall — no session named — still sees it.
    let project_wide = flags_to_scope("repo", None, None);
    assert!(
        project_wide.admits(&written_at),
        "a project-wide recall reaches the session-scoped row"
    );
}

// ---- the session JSON arriving on stdin ----

const A_SESSION: &str = r#"{
  "episodes": [
    { "key": "episode:1", "text": "The build broke on the lockfile.", "turn_id": "turn-7" }
  ],
  "rules": [
    { "key": "rule:lockfile", "text": "Building needs cargo fetch.", "derived_from": "episode:1" }
  ],
  "procedures": [
    {
      "key": "procedure:release",
      "title": "Cut a release",
      "body": "1. run the tests\n2. tag the commit",
      "inputs": ["a clean tree"],
      "expected": "a green build",
      "derived_from": "rule:lockfile",
      "used_count": 0,
      "last_outcome": null
    }
  ]
}"#;

#[test]
fn a_session_parses_from_the_json_the_harness_sends() {
    let record: SessionRecord =
        serde_json::from_str(A_SESSION).expect("the documented session format parses");
    assert_eq!(record.episodes.len(), 1);
    assert_eq!(record.rules.len(), 1);
    assert_eq!(record.procedures.len(), 1);
    // A rule names the episode it came from by that episode's key.
    assert_eq!(record.rules[0].derived_from.as_deref(), Some("episode:1"));
    // A procedure names its rule.
    assert_eq!(
        record.procedures[0].derived_from.as_deref(),
        Some("rule:lockfile")
    );
}

#[test]
fn an_empty_session_parses_to_an_empty_record() {
    let record: SessionRecord = serde_json::from_str("{}").expect("an empty object parses");
    assert!(record.episodes.is_empty());
    assert!(record.rules.is_empty());
    assert!(record.procedures.is_empty());
}

#[test]
fn a_session_field_missing_is_absent_not_an_error() {
    // Every sequence is optional: a harness that only learned a rule sends just
    // that, and a writer that has nothing yet sends {}.
    let record: SessionRecord =
        serde_json::from_str(r#"{"rules":[{"key":"rule:x","text":"Holds."}]}"#)
            .expect("a rules-only session parses");
    assert_eq!(record.rules.len(), 1);
    assert!(record.episodes.is_empty());
    // The optional link is absent, which the writer turns into no edge.
    assert_eq!(record.rules[0].derived_from, None);
}

#[test]
fn the_published_field_names_are_what_the_reader_expects() {
    // Renaming a field is a breaking change for whoever pipes this in, so the
    // names are asserted rather than trusted.
    let value: serde_json::Value = serde_json::from_str(A_SESSION).unwrap();
    let episode = &value["episodes"][0];
    for field in ["key", "text", "turn_id"] {
        assert!(episode.get(field).is_some(), "episode.{field} must exist");
    }
    let rule = &value["rules"][0];
    for field in ["key", "text", "derived_from"] {
        assert!(rule.get(field).is_some(), "rule.{field} must exist");
    }
    let procedure = &value["procedures"][0];
    for field in [
        "key",
        "title",
        "body",
        "inputs",
        "expected",
        "derived_from",
        "used_count",
        "last_outcome",
    ] {
        assert!(
            procedure.get(field).is_some(),
            "procedure.{field} must exist"
        );
    }
}

#[test]
fn a_session_round_trips_through_json() {
    // So a harness can take what `recall` printed and feed it back to `remember`
    // when that is what it wants.
    let record: SessionRecord = serde_json::from_str(A_SESSION).unwrap();
    let again: SessionRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap())
        .expect("the written form parses back");
    assert_eq!(record, again);
}

// ---- the command line ----

#[test]
fn the_write_command_takes_a_session_on_stdin() {
    // Shape only: the flags a harness passes for a write.
    let cli = Cli::parse_from([
        "coding-remember",
        "--store",
        "/tmp/fact-store",
        "remember",
        "--project",
        "repo",
        "--session",
        "exec-7",
    ]);
    assert_eq!(cli.store, std::path::PathBuf::from("/tmp/fact-store"));
    match cli.command {
        Command::Remember {
            project, session, ..
        } => {
            assert_eq!(project, "repo");
            assert_eq!(session, "exec-7");
        }
        _ => panic!("expected a write"),
    }
}

#[test]
fn the_recall_command_does_not_need_a_session() {
    let cli = Cli::parse_from([
        "coding-remember",
        "--store",
        "/tmp/fact-store",
        "recall",
        "--project",
        "repo",
    ]);
    match cli.command {
        Command::Recall {
            project, session, ..
        } => {
            assert_eq!(project, "repo");
            assert!(session.is_none(), "no session means the whole project");
        }
        _ => panic!("expected a recall"),
    }
}

#[test]
fn the_edges_command_needs_only_the_project() {
    let cli = Cli::parse_from([
        "coding-remember",
        "--store",
        "s",
        "edges",
        "--project",
        "repo",
    ]);
    match cli.command {
        Command::Edges { project } => assert_eq!(project, "repo"),
        _ => panic!("expected edges"),
    }
}

#[test]
fn the_recall_kind_is_optional() {
    // `all` is the default, so a harness that wants everything passes nothing.
    let cli = Cli::parse_from([
        "coding-remember",
        "--store",
        "s",
        "recall",
        "--project",
        "r",
    ]);
    match cli.command {
        Command::Recall { kind, .. } => assert_eq!(kind, "all"),
        _ => panic!("expected a recall"),
    }
}
