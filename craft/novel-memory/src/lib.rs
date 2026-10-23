//! The novel craft: bend the generic memory engine toward fiction.
//!
//! The narrative kernel — story-time coordinates and slot hashing — lives in
//! `narrative-core`, which the tavern craft sits on too. This crate adds what
//! is specific to a novel: the corpus scan, the three-store assembly, the
//! read paths and the subcommands.
//!
//! The three layers, and where each comes from:
//!
//! | layer | source | how it lands |
//! |---|---|---|
//! | setting | concept cards | one card-level fact per card, plus one fact per section |
//! | fact | drafts, plans, story outlines | one fact per state change |
//! | relation | relation declarations on cards | an edge, plus a declaration fact and an evolution fact |
//!
//! The timeline is **story time**, not writing order: fiction is ordered by
//! chapter, but flashbacks make writing order useless for reasoning. Facts
//! therefore carry `valid_from` (a story number), and the scope's `task_id` is
//! zero-padded so lexicographic order equals story order.

pub mod cli;
pub mod config;
pub mod corpus;
pub mod query;
pub mod store;

// The kernel is re-exported, so a consumer of the novel craft still finds
// `novel_memory::storytime::*` and `novel_memory::hash::*` where it did
// before the kernel became its own crate.
pub use narrative_core::hash;
pub use narrative_core::storytime;

pub use cli::{run, MemoryCmd};
pub use config::{Config, Paths};
pub use corpus::{scan, CardInfo, Corpus, EdgeSlot, FactSlot};
pub use hash::{content_hash, event_id, hash_of_event_id, slot_of_event_id};
pub use query::{edge_line, relations, timeline, Hit};
pub use store::{IndexReport, Indexer, MemoryBackend};
pub use storytime::{
    chapter_beat, format_tick, format_tick_label, identity_of_unit_name, parse_marker, parse_tick,
    parse_unit_name, tick_of_identity, Layer, StoryTimeMarker, StoryUnit, Tick, TICK_SCALE,
};
