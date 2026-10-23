//! Content hashing: decide whether a slot's text changed.
//!
//! A fact id is the **slot key** (a position) and must not carry a content
//! hash — if the id moves, `supersede` can no longer find the old row and the
//! correction chain breaks. So "did it change" is recorded elsewhere: the
//! content hash goes into `SourceRef.event_id`, and a rescan reads it back and
//! compares.
//!
//! This is part of the narrative kernel: both the novel craft (draft slots) and
//! the tavern craft (session slots) need the same slot-key-plus-hash scheme.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// The content hash of a text (hexadecimal).
///
/// `DefaultHasher`, not MD5/SHA: this only needs "same text → same value" and
/// "different text → very likely a different value". There is no adversarial
/// input, and the hash is only ever compared inside one library — never across
/// libraries or versions.
#[must_use]
pub fn content_hash(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Join a slot key and a content hash into a `SourceRef.event_id`.
///
/// The slot key comes first so the event id shows which slot it is; the hash
/// follows for comparison.
#[must_use]
pub fn event_id(slot_key: &str, text: &str) -> String {
    format!("{slot_key}#{}", content_hash(text))
}

/// Recover the slot-key part from a `SourceRef.event_id`.
///
/// The shape is `slot-key#content-hash`; an id without `#` (written by
/// something else) is returned unchanged, so callers lose nothing by treating
/// it as a slot key.
#[must_use]
pub fn slot_of_event_id(event_id: &str) -> &str {
    event_id.rsplit_once('#').map_or(event_id, |(slot, _)| slot)
}

/// Recover the content-hash part from a `SourceRef.event_id`.
///
/// `None` when the id has no `#` (the older format) — that id was written by
/// something else, not by this module's slots.
#[must_use]
pub fn hash_of_event_id(event_id: &str) -> Option<&str> {
    event_id.rsplit_once('#').map(|(_, hash)| hash)
}
