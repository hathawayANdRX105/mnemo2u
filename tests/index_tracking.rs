//! Tracking-row rules — `apply_source_ids_limit` and the relation key helpers
//! (`src/index/tracking.rs`).

use mnemo2u::index::tracking::{
    apply_source_ids_limit, make_relation_chunk_key, merge_source_ids, parse_relation_chunk_key,
    LIMIT_FIFO, LIMIT_KEEP,
};

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn keep_strategy_drops_the_tail() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(
        apply_source_ids_limit(&list, 2, LIMIT_KEEP),
        ids(&["a", "b"])
    );
}

#[test]
fn fifo_strategy_drops_the_head() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(
        apply_source_ids_limit(&list, 2, LIMIT_FIFO),
        ids(&["b", "c"])
    );
}

#[test]
fn unknown_method_falls_back_to_keep() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(
        apply_source_ids_limit(&list, 2, "SOMETHING"),
        ids(&["a", "b"])
    );
}

#[test]
fn limits_are_case_insensitive() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(apply_source_ids_limit(&list, 2, "fifo"), ids(&["b", "c"]));
}

#[test]
fn zero_limit_disables_the_cap() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(apply_source_ids_limit(&list, 0, LIMIT_KEEP), list);
}

#[test]
fn limit_above_length_is_a_no_op() {
    let list = ids(&["a", "b"]);
    assert_eq!(apply_source_ids_limit(&list, 5, LIMIT_FIFO), list);
}

#[test]
fn relation_key_orders_the_pair() {
    assert_eq!(make_relation_chunk_key("BETA", "ACME"), "ACME<SEP>BETA");
    assert_eq!(make_relation_chunk_key("ACME", "BETA"), "ACME<SEP>BETA");
}

#[test]
fn relation_key_round_trip() {
    let key = make_relation_chunk_key("GAMMA", "DELTA");
    assert_eq!(
        parse_relation_chunk_key(&key),
        Some(("GAMMA".to_string(), "DELTA".to_string()))
    );
}

#[test]
fn malformed_relation_key_is_rejected() {
    assert_eq!(parse_relation_chunk_key("ACME"), None);
    assert_eq!(parse_relation_chunk_key("ACME<SEP>BETA<SEP>GAMMA"), None);
}

#[test]
fn merge_keeps_existing_order_then_caps() {
    let merged = merge_source_ids(&ids(&["a", "b", "c"]), &ids(&["c", "d"]), 10, LIMIT_KEEP);
    assert_eq!(merged, ids(&["a", "b", "c", "d"]));
}

#[test]
fn merge_cap_applies_after_the_union() {
    let merged = merge_source_ids(&ids(&["a", "b", "c"]), &ids(&["d"]), 2, LIMIT_FIFO);
    assert_eq!(merged, ids(&["c", "d"]));
}

#[test]
fn merge_drops_empty_ids() {
    let merged = merge_source_ids(&ids(&["a", ""]), &ids(&["", "b"]), 10, LIMIT_KEEP);
    assert_eq!(merged, ids(&["a", "b"]));
}
