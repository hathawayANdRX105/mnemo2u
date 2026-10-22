//! Byte-parity: entity vector row shape vs LightRAG `operate.py:2746`
//! (`f"{entity_name}\n{description}"` content) and the documented meta
//! defaults. Pins the R2 `{name}\n{description}` content join.

use mnemo2u::core::text::compute_mdhash_id;
use mnemo2u::index::merge::entity_vector_rows;
use serde_json::{json, Value};

#[test]
fn entity_row_content_is_name_newline_description() {
    let node: Value = json!({
        "entity_name": "ACME",
        "entity_type": "organization",
        "description": "ACME makes cobots.",
        "source_id": "chunk-a",
        "file_path": "docs/a.md",
        "created_at": 42
    });
    let rows = entity_vector_rows(std::slice::from_ref(&node));
    assert_eq!(rows.len(), 1);
    // The R2 byte-parity detail: name and description joined by a newline
    // (not concatenated, not space-joined).
    assert_eq!(rows[0].content, "ACME\nACME makes cobots.");
    assert_eq!(rows[0].id, compute_mdhash_id("ACME", "ent-"));
    assert_eq!(
        rows[0].meta,
        json!({
            "entity_name": "ACME",
            "entity_type": "organization",
            "source_id": "chunk-a",
            "file_path": "docs/a.md",
            "created_at": 42
        })
    );
}

#[test]
fn entity_row_meta_fills_the_documented_defaults() {
    // Only name + description present: entity_type/source_id/file_path/created_at
    // fall back to the documented defaults (UNKNOWN / empty / unknown_source / 0).
    let node: Value = json!({ "entity_name": "BETA", "description": "BETA builds drones." });
    let rows = entity_vector_rows(std::slice::from_ref(&node));
    assert_eq!(rows[0].content, "BETA\nBETA builds drones.");
    assert_eq!(
        rows[0].meta,
        json!({
            "entity_name": "BETA",
            "entity_type": "UNKNOWN",
            "source_id": "",
            "file_path": "unknown_source",
            "created_at": 0
        })
    );
}
