//! The narrative kernel: the pieces every fiction craft sits on.
//!
//! Two things live here, and both are about **position** rather than content:
//!
//! - **story time** ([`storytime`]): where an event sits in the fiction's
//!   timeline. Fiction is not written in story order, so chapter order cannot
//!   be the timeline; a tavern transcript is written in order, but the same
//!   coordinate system serves both.
//! - **slot hashing** ([`hash`]): whether a slot's text changed. A fact id is
//!   the slot's *position*, so it must not move when the text does — the
//!   change is recorded in the source reference instead.
//!
//! The crafts build on this: the novel craft scans a novel directory into
//! fact slots, the tavern craft scans a session into canon and events. Neither
//! owns these two pieces; both use them.

pub mod hash;
pub mod storytime;

pub use hash::{content_hash, event_id, hash_of_event_id, slot_of_event_id};
pub use storytime::{
    chapter_beat, format_tick, format_tick_label, identity_of_unit_name, parse_marker, parse_tick,
    parse_unit_name, tick_of_identity, Layer, StoryTimeMarker, StoryUnit, Tick, TICK_SCALE,
};
