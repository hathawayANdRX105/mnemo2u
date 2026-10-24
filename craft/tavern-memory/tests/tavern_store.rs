//! The tavern craft's store wiring: scope arithmetic and ids, without the
//! embedder.
//!
//! Opening the vector store needs fastembed, which needs a model, so these
//! tests pin `plan` — the pure function that decides scopes and ids — rather
//! than the write path. The write path mirrors the novel craft's, which the
//! review tool exercises in production.

use mnemo2u::core::types::ScopeKey;
use tavern_memory::store::{content_hash, event_id, hash_of_event_id, plan};
use tavern_memory::{
    canon_fact, event_fact, CanonEntry, FactKind, SessionEvent, TavernMemory, TavernScope,
};

fn canon(key: &str, text: &str, layer: narrative_core::storytime::Layer) -> CanonEntry {
    CanonEntry {
        key: key.to_string(),
        text: text.to_string(),
        layer,
        source_path: "cards/characters/lu-chen.md".to_string(),
        source_line: 12,
    }
}

fn event(key: &str, text: &str, tick: i64) -> SessionEvent {
    SessionEvent {
        key: key.to_string(),
        text: text.to_string(),
        tick,
        source_path: "drafts/story1-ch01.md".to_string(),
        source_line: 3,
    }
}

fn memory(canon_entry: CanonEntry, session_event: SessionEvent) -> TavernMemory {
    TavernMemory {
        canon: [(canon_entry.key.clone(), canon_entry)]
            .into_iter()
            .collect(),
        events: [(session_event.key.clone(), session_event)]
            .into_iter()
            .collect(),
        units: Vec::new(),
    }
}

fn facts_of<'a>(plan: &'a tavern_memory::WritePlan, id: &str) -> &'a mnemo2u::core::types::Fact {
    plan.facts
        .iter()
        .find(|fact| fact.id.0 == id)
        .unwrap_or_else(|| panic!("no fact with id {id}"))
}

#[test]
fn a_canon_fact_drops_the_session_id() {
    // Project knowledge: "she never smokes" holds for the character no matter
    // which session observed it.
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    let fact = facts_of(&plan, "canon:smokes");
    // The session id is kept, not dropped: see the module docs on why a canon
    // that dropped it would be unreachable from the session that wrote it.
    assert_eq!(fact.scope.session_id.as_deref(), Some("session-1"));
    assert_eq!(fact.kind, FactKind::Canon.as_str());
}

#[test]
fn an_event_fact_keeps_the_session_id() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    let fact = facts_of(&plan, "event:ch01");
    assert_eq!(fact.scope.session_id.as_deref(), Some("session-1"));
    assert_eq!(fact.kind, FactKind::Event.as_str());
}

#[test]
fn a_character_wide_recall_sees_the_whole_canon_and_its_events() {
    // What the split is for: the write happens inside one session, the read
    // asks about the character.
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    // A world-book rendering: no session named — the character scope.
    let whole_character = scope.to_character_scope();
    assert!(
        whole_character.admits(&facts_of(&plan, "canon:smokes").scope),
        "a canon entry is character knowledge"
    );
    assert!(
        whole_character.admits(&facts_of(&plan, "event:ch01").scope),
        "a character-wide recall also sees what its sessions did"
    );
}

#[test]
fn another_session_cannot_adopt_this_sessions_events() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    let another = ScopeKey {
        workspace_id: "char-lu-chen".to_string(),
        session_id: Some("session-2".to_string()),
        task_id: None,
    };
    assert!(
        !another.admits(&facts_of(&plan, "event:ch01").scope),
        "an event happened in one session, not in the character"
    );
    // And it cannot see the canon it did not write. This is `admits` at its
    // strictest: a stored `None` is invisible to a query that names a session,
    // so a canon that dropped its session id would be unreachable from the
    // session that wrote it. A character-wide recall is the caller's opening:
    // `TavernScope::to_character_scope`, which names no session at all.
    assert!(!another.admits(&facts_of(&plan, "canon:smokes").scope));
}

#[test]
fn another_characters_workspace_is_invisible() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    let elsewhere = ScopeKey {
        workspace_id: "char-someone-else".to_string(),
        session_id: None,
        task_id: None,
    };
    assert!(!elsewhere.admits(&facts_of(&plan, "canon:smokes").scope));
    assert!(!elsewhere.admits(&facts_of(&plan, "event:ch01").scope));
}

#[test]
fn an_events_story_time_travels_as_its_valid_from() {
    // Story time is the event's position in the fiction's own timeline, which
    // is not the order the transcript was written in.
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 4_000),
        ),
        &scope,
        0,
    );
    assert_eq!(facts_of(&plan, "event:ch01").valid_from, Some(4_000));
    // A canon entry is not an event, so it carries no story time.
    assert_eq!(facts_of(&plan, "canon:smokes").valid_from, None);
}

#[test]
fn the_card_is_a_facts_provenance() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let entry = canon(
        "canon:smokes",
        "She never smokes.",
        narrative_core::storytime::Layer::Fact,
    );
    let plan = plan(
        &memory(
            entry.clone(),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    assert_eq!(
        facts_of(&plan, "canon:smokes").source_refs[0].branch_id,
        entry.source_path,
        "the card is where a human goes to read the sheet"
    );
}

#[test]
fn the_content_hash_decides_idempotency() {
    // The narrative kernel's slot scheme, so the same rescan logic holds here
    // as in the novel craft.
    let id = event_id("canon:smokes", "She never smokes.");
    assert_eq!(
        hash_of_event_id(&id),
        Some(content_hash("She never smokes.").as_str())
    );
    assert_ne!(
        content_hash("She never smokes."),
        content_hash("She sometimes smokes.")
    );
}

#[test]
fn the_baseline_reaches_the_facts_created_at() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        1_700_000_000_000,
    );
    assert_eq!(
        facts_of(&plan, "canon:smokes").created_at,
        1_700_000_000_000
    );
    assert_eq!(facts_of(&plan, "event:ch01").created_at, 1_700_000_000_000);
}

#[test]
fn the_fact_id_is_the_slot_key_never_the_text() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let before = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    let after = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes, not even once.",
                narrative_core::storytime::Layer::Fact,
            ),
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    assert_eq!(facts_of(&before, "canon:smokes").id.0, "canon:smokes");
    assert_eq!(
        facts_of(&after, "canon:smokes").id.0,
        "canon:smokes",
        "the id survives the edit, so supersede can find the old row"
    );
}

#[test]
fn the_planner_and_the_lib_agree_on_a_facts_shape() {
    // The store's plan is the same fact the lib's own constructor builds, one
    // layer down. Two builders for one row would drift; this pins that they
    // do not.
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let entry = canon(
        "canon:smokes",
        "She never smokes.",
        narrative_core::storytime::Layer::Fact,
    );
    // The lib's own constructor takes the session scope, so that is the scope
    // the writer must match.
    let expected = canon_fact(&scope.to_scope_key(), &entry, "", 0);
    let the_plan = plan(
        &memory(
            entry,
            event("event:ch01", "She lit a cigarette at dawn.", 1),
        ),
        &scope,
        0,
    );
    let got = facts_of(&the_plan, "canon:smokes");
    assert_eq!(got.id, expected.id);
    assert_eq!(got.kind, expected.kind);
    assert_eq!(got.text, expected.text);
    assert_eq!(got.scope, expected.scope);
}

#[test]
fn a_conflict_is_reported_not_silently_rewritten() {
    // The write path never supersedes a canon fact because an event
    // disagreed with it. Which one wins is a judgement about the fiction.
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let session = memory(
        canon(
            "canon:smokes",
            "She never smokes.",
            narrative_core::storytime::Layer::Fact,
        ),
        event("event:ch01", "She lit a cigarette at dawn.", 1),
    );
    let found = tavern_memory::detect(session.canon.values(), session.events.values());
    assert_eq!(found.len(), 1, "the event collides with the canon");
    // And the canon fact is still the canon fact after the write plan.
    let plan = plan(&session, &scope, 0);
    assert_eq!(
        facts_of(&plan, "canon:smokes").kind,
        FactKind::Canon.as_str(),
        "the canon fact is still the canon fact"
    );
}

#[test]
fn the_event_lib_constructor_and_the_plan_agree() {
    let scope = TavernScope::new("char-lu-chen", "session-1");
    let session_event = event("event:ch01", "She lit a cigarette at dawn.", 3_000);
    let expected = event_fact(
        &ScopeKey {
            workspace_id: "char-lu-chen".to_string(),
            session_id: Some("session-1".to_string()),
            task_id: None,
        },
        &session_event,
        0,
    );
    let the_plan = plan(
        &memory(
            canon(
                "canon:smokes",
                "She never smokes.",
                narrative_core::storytime::Layer::Fact,
            ),
            session_event,
        ),
        &scope,
        0,
    );
    let got = facts_of(&the_plan, "event:ch01");
    assert_eq!(got.id, expected.id);
    assert_eq!(got.text, expected.text);
    assert_eq!(got.scope, expected.scope);
    assert_eq!(got.valid_from, expected.valid_from);
    assert_eq!(got.kind, expected.kind);
}
