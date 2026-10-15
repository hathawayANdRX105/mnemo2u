//! Post-retrieval conversation / time filtering contract for `QueryParam`
//! (FAC6 / R3.T5 gap closure).
//!
//! The RAG arms apply the filter after retrieval and before render:
//! `kg_query` filters chunk vector hits and graph records, `naive_query`
//! filters chunk vector hits. Both converge on the predicate tested here
//! (`query::kg::chunk_hit_admits`); the answer-cache key
//! (`query::keywords::answer_cache_inputs`) must fold the two new fields in
//! or distinct filters would share a context. No LLM/vector backend is
//! needed, so the suite is deterministic and offline.

use serde_json::{json, Value};

use mnemo2u::core::rag::{QueryMode, QueryParam, TimeWindow};
use mnemo2u::core::traits::VectorHit;
use mnemo2u::query::keywords::answer_cache_inputs;
use mnemo2u::query::kg::chunk_hit_admits;

/// A chunk vector hit whose metadata carries the conversation + timestamp
/// carrier that session/turn ingestion merges into the row
/// (`index::ingest::META_CONVERSATION_ID` / `META_TIMESTAMP`).
fn hit(id: &str, conversation_id: Option<&str>, timestamp_secs: Option<i64>) -> VectorHit {
    let mut meta = json!({});
    if let Some(conversation_id) = conversation_id {
        meta["conversation_id"] = json!(conversation_id);
    }
    if let Some(timestamp_secs) = timestamp_secs {
        meta["timestamp"] = json!(timestamp_secs);
    }
    VectorHit {
        id: id.to_string(),
        distance: 0.0,
        meta,
    }
}

/// `None` filters everywhere → current behaviour, every hit passes.
fn no_filter() -> QueryParam {
    QueryParam::default()
}

// --- 1. Conversation filter drops the other session's records. ---

#[test]
fn conversation_filter_drops_other_sessions_records() {
    let a = hit("a", Some("A"), None);
    let b = hit("b", Some("B"), None);
    let param = QueryParam {
        conversation_id: Some("A".to_string()),
        ..QueryParam::default()
    };

    // The filter must keep A and reject B; an AND/OR inversion or a
    // forgotten filter would let B through.
    assert!(chunk_hit_admits(&a, &param), "A should admit");
    assert!(!chunk_hit_admits(&b, &param), "B must be dropped");
    // Without the filter, both pass (current behaviour unchanged).
    assert!(chunk_hit_admits(&a, &no_filter()));
    assert!(chunk_hit_admits(&b, &no_filter()));
}

// --- 2. Time-window bounds are inclusive and single-sided. ---

#[test]
fn time_window_bounds_are_inclusive_and_single_sided() {
    // `META_TIMESTAMP` is unix seconds; `TimeWindow` bounds are ms, so the
    // carrier is scaled by 1000 before comparison. Use seconds and assert
    // against the scaled millisecond value.
    let at_1s = 1_000; // == 1_000_000 ms
    let at_2s = 2_000; // == 2_000_000 ms
    let at_3s = 3_000; // == 3_000_000 ms

    // Since-only: inclusive lower bound; below is dropped.
    let since = TimeWindow {
        since_ms: Some(at_1s * 1000),
        until_ms: None,
    };
    assert!(since.admits(at_1s * 1000), "boundary =since retained");
    assert!(since.admits(at_2s * 1000));
    assert!(
        !since.admits((at_1s * 1000) - 1),
        "just below since dropped"
    );

    // Until-only: inclusive upper bound; above is dropped.
    let until = TimeWindow {
        since_ms: None,
        until_ms: Some(at_2s * 1000),
    };
    assert!(until.admits(at_2s * 1000), "boundary =until retained");
    assert!(until.admits(at_1s * 1000));
    assert!(
        !until.admits((at_2s * 1000) + 1),
        "just above until dropped"
    );

    // Both-sided: inclusive on both ends.
    let both = TimeWindow {
        since_ms: Some(at_1s * 1000),
        until_ms: Some(at_2s * 1000),
    };
    assert!(both.admits(at_1s * 1000));
    assert!(both.admits(at_2s * 1000));
    assert!(both.admits(at_3s / 2 * 1000)); // 1.5s inside
    assert!(!both.admits((at_1s * 1000) - 1));
    assert!(!both.admits((at_2s * 1000) + 1));

    // `None` on both sides = unbounded = behaves like no window.
    let open = TimeWindow::default();
    assert!(open.admits(at_1s * 1000));
    assert!(open.admits(0));

    // Contradictory window (since > until) admits nothing — a self-
    // contradicting filter is not an error, it yields no record.
    let contradictory = TimeWindow {
        since_ms: Some(at_2s * 1000),
        until_ms: Some(at_1s * 1000),
    };
    assert!(!contradictory.admits(at_1s * 1000));
    assert!(!contradictory.admits(at_2s * 1000));
    assert!(!contradictory.admits(at_3s * 1000));

    // Through the hit path: a missing/zero timestamp record is dropped
    // under a non-empty window (宁缺勿假).
    let with_ts = hit("x", None, Some(at_1s));
    let without_ts = hit("y", None, None);
    assert!(chunk_hit_admits(
        &with_ts,
        &QueryParam {
            time_window: Some(both),
            ..QueryParam::default()
        }
    ));
    assert!(!chunk_hit_admits(
        &without_ts,
        &QueryParam {
            time_window: Some(both),
            ..QueryParam::default()
        }
    ));
}

// --- 3. Missing-metadata rows are dropped only under a filter. ---

#[test]
fn missing_metadata_rows_are_dropped_only_under_filter() {
    let bare = hit("bare", None, None);

    // No filter: current behaviour, the row is kept.
    assert!(chunk_hit_admits(&bare, &no_filter()));

    // Conversation filter active but the row carries no conversation_id:
    // uncertain provenance must not leak into a narrowed result.
    assert!(!chunk_hit_admits(
        &bare,
        &QueryParam {
            conversation_id: Some("A".to_string()),
            ..QueryParam::default()
        }
    ));

    // Time filter active but the row carries no timestamp: same rule.
    assert!(!chunk_hit_admits(
        &bare,
        &QueryParam {
            time_window: Some(TimeWindow {
                since_ms: Some(0),
                until_ms: Some(i64::MAX),
            }),
            ..QueryParam::default()
        }
    ));
}

// --- 4. Filter values enter the answer-cache key. ---

#[test]
fn filter_values_enter_the_answer_cache_key() {
    let base = QueryParam {
        mode: QueryMode::Mix,
        ..QueryParam::default()
    };
    let conv_a = QueryParam {
        conversation_id: Some("A".to_string()),
        ..base.clone()
    };
    let conv_b = QueryParam {
        conversation_id: Some("B".to_string()),
        ..base.clone()
    };
    let windowed = QueryParam {
        time_window: Some(TimeWindow {
            since_ms: Some(1_000),
            until_ms: Some(2_000),
        }),
        ..base.clone()
    };

    let key_base = answer_cache_inputs(&base);
    let key_a = answer_cache_inputs(&conv_a);
    let key_b = answer_cache_inputs(&conv_b);
    let key_windowed = answer_cache_inputs(&windowed);

    // Distinct conversation filters must not share a cache key — the key
    // bug would omit the field and make A's context serve B's query.
    assert_ne!(key_a, key_b, "different conversation_ids => different keys");
    assert!(
        key_a.iter().any(|part| part.contains('A')) && key_b.iter().any(|part| part.contains('B')),
        "conversation id must appear in the key"
    );
    // A time window also changes the key (and differs from no window).
    assert_ne!(key_windowed, key_base, "time window must change the key");
    assert!(
        key_windowed
            .iter()
            .any(|part| part.contains("since=1000") && part.contains("until=2000")),
        "time window must appear in the key"
    );
}
#[test]
fn query_param_new_fields_have_defaults() {
    // `..QueryParam::default()` must still compile at every existing
    // construction site; the new fields default to `None`.
    let param = QueryParam {
        mode: QueryMode::Mix,
        ..QueryParam::default()
    };
    assert_eq!(param.conversation_id, None);
    assert_eq!(param.time_window, None);

    // A legacy payload (none of the new fields) must still parse and
    // yield `None` — serde `default` keeps old payloads compatible.
    let mut legacy = serde_json::to_value(QueryParam::default()).expect("default serializes");
    let object = legacy.as_object_mut().expect("value is an object");
    object.remove("conversation_id");
    object.remove("time_window");
    let legacy: QueryParam =
        serde_json::from_value(legacy).expect("legacy payload (no new fields) parses");
    assert_eq!(legacy.conversation_id, None);
    assert_eq!(legacy.time_window, None);

    // A payload that does set them round-trips.
    let rich: QueryParam = serde_json::from_value(json!({
        "mode": "mix",
        "conversation_id": "A",
        "time_window": {"since_ms": 1, "until_ms": 2}
    }))
    .expect("rich payload parses");
    assert_eq!(rich.conversation_id.as_deref(), Some("A"));
    assert_eq!(
        rich.time_window,
        Some(TimeWindow {
            since_ms: Some(1),
            until_ms: Some(2)
        })
    );
}
