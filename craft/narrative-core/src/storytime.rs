//! Story-time coordinates: turn "which story, which beat inside it" into a
//! sortable coordinate.
//!
//! Ordering fiction by chapter destroys the reasoning: flashbacks and
//! non-linear chapters mean writing order says nothing about story order. The
//! timeline here is **story time** — an integer story number plus the beat
//! inside that story.
//!
//! Coordinates are fixed-point integers ([`TICK_SCALE`] = thousandths of a
//! beat), never floats:
//!
//! - `story-1-ambush` → story 1 → tick 1000;
//! - `interlude1` → sits after story 1 → tick 1500;
//! - `<!-- story-time: 1.5 -->` → explicit marker → tick 1500.
//!
//! Several units inside one story (`story-5a-…`, `story-5b-…`) share the story
//! number and are ordered by their unit identity.
//!
//! This is the **narrative kernel**: the piece the novel craft and the tavern
//! craft both sit on. A tavern session is a story too — its events need the
//! same story-time ordering, and its canon is the same three layers.

use std::fmt;

/// Fixed-point precision of one beat: the integer part of a tick is the story
/// number, the fraction places interludes.
pub const TICK_SCALE: i64 = 1000;

/// A story-time coordinate (millibeats).
pub type Tick = i64;

/// One story unit: a story or an interlude.
///
/// The unit's **identity** ([`StoryUnit::identity`], e.g. `story-1` /
/// `interlude-1`) is the key used to align across directories: the same unit is
/// called `interlude1` in the draft tree and `interlude-1-ground-level-explorer.md`
/// in the outline tree — same identity, same time, same scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryUnit {
    /// Unit identity (`story-1` / `story-5a` / `interlude-1`).
    pub identity: String,
    /// Story-time coordinate.
    pub tick: Tick,
    /// Beat inside the story (chapter number); 0 means unit-level content.
    pub beat: u32,
    /// Human-readable name (directory or file stem).
    pub display: String,
}

impl StoryUnit {
    /// The `task_id` component of a scope: zero-padded so lexicographic order
    /// equals story order.
    ///
    /// `story:001` (story 1) < `story:001.500` (the interlude after story 1) <
    /// `story:002` (story 2). Both the integer and the fraction are padded to
    /// three digits; whole beats carry no fraction, and `-` sorts before `.`,
    /// so the whole-beat unit precedes fractional ones under the same prefix.
    pub fn task_id(&self) -> String {
        format!("story:{}", format_tick(self.tick))
    }

    /// Human-readable story label: `story 1` / `story 1.5`.
    pub fn label(&self) -> String {
        format!("story {}", format_tick_label(self.tick))
    }
}

/// The zero-padded form of a tick (used for scope `task_id`).
pub fn format_tick(tick: Tick) -> String {
    let integer = tick / TICK_SCALE;
    let fraction = tick % TICK_SCALE;
    if fraction == 0 {
        format!("{integer:03}")
    } else {
        format!("{integer:03}.{fraction:03}")
    }
}

/// The human-readable form of a tick (`1` / `1.5`).
pub fn format_tick_label(tick: Tick) -> String {
    let integer = tick / TICK_SCALE;
    let fraction = tick % TICK_SCALE;
    if fraction == 0 {
        integer.to_string()
    } else {
        format!("{integer}.{fraction:03}")
    }
}

/// An explicit time marker: `<!-- story-time: 1.5 -->` in a draft or a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoryTimeMarker {
    /// Story-time coordinate.
    pub tick: Tick,
    /// Explicit beat (defaults to the chapter number).
    pub beat: Option<u32>,
}

/// Parse an explicit marker out of text.
///
/// The format is an HTML comment carrying a `story-time:` key and an optional
/// `beat:` key:
///
/// ```text
/// <!-- story-time: 1.5 -->
/// <!-- story-time: 1.5 beat: 4 -->
/// ```
///
/// A comment is the only construct in Markdown that neither renders nor leaks
/// into the prose, so it works both at the top of a chapter file and in a unit
/// directory's `README.md` as a unit-level annotation.
pub fn parse_marker(text: &str) -> Option<StoryTimeMarker> {
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start + 4..];
        let Some(end) = after.find("-->") else {
            break;
        };
        if let Some(marker) = parse_marker_body(&after[..end]) {
            return Some(marker);
        }
        rest = &after[end + 3..];
    }
    None
}

/// Parse a marker out of one comment body.
fn parse_marker_body(body: &str) -> Option<StoryTimeMarker> {
    let tick = parse_after_key(body, "story-time").and_then(|value| parse_tick(&value))?;
    let beat = parse_after_key(body, "beat").and_then(|value| value.trim().parse().ok());
    Some(StoryTimeMarker { tick, beat })
}

/// Take the value text that follows `key:`.
fn parse_after_key(body: &str, key: &str) -> Option<String> {
    let position = body.find(key)?;
    let after = &body[position + key.len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();
    let end = value
        .find(|character: char| character.is_whitespace())
        .unwrap_or(value.len());
    Some(value[..end].to_string())
}

/// Parse decimal text into a fixed-point tick (`1` → 1000, `1.5` → 1500).
///
/// Not floats: decimal round-tripping through floats is inexact and would make
/// the ordering unstable. At most three fraction digits.
pub fn parse_tick(text: &str) -> Option<Tick> {
    let text = text.trim();
    let (integer_part, fraction_part) = match text.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (text, None),
    };
    if integer_part.is_empty() || !integer_part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let integer: i64 = integer_part.parse().ok()?;
    let fraction = match fraction_part {
        None => 0,
        Some(part) => {
            if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            // Pad on the right: `5` means 0.5 beats, not 0.005 beats.
            let padded = format!("{part:0<3}");
            padded.parse().ok()?
        }
    };
    Some(integer * TICK_SCALE + fraction)
}

/// Unit name → unit identity and story-time coordinate.
///
/// - `story-1-ambush` / `story1` / `story-5a-…` → story number (`story-5a`
///   belongs to story 5, sharing the number with `story-5b` but not the
///   identity);
/// - `interlude1` / `interlude-1-…` → sits half a beat after that story.
///
/// An unrecognised name yields `None`: it is not a story unit and stays off the
/// story timeline.
pub fn parse_unit_name(name: &str) -> Option<(String, Tick)> {
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("story") {
        let (digits, letter) = split_number(rest)?;
        let identity = if letter.is_empty() {
            format!("story-{digits}")
        } else {
            format!("story-{digits}{letter}")
        };
        let number: i64 = digits.parse().ok()?;
        return Some((identity, number * TICK_SCALE));
    }
    if let Some(rest) = lower.strip_prefix("interlude") {
        let (digits, _) = split_number(rest)?;
        let number: i64 = digits.parse().ok()?;
        return Some((
            format!("interlude-{digits}"),
            number * TICK_SCALE + TICK_SCALE / 2,
        ));
    }
    None
}

/// Unit identity → story-time coordinate.
///
/// The identity is the stable post-parse key (`story-1` / `story-5a` /
/// `interlude-1`); look the time up by it when aligning across directories.
pub fn tick_of_identity(identity: &str) -> Option<Tick> {
    parse_unit_name(identity).map(|(_, tick)| tick)
}

/// Unit name → unit identity, without resolving the time.
///
/// The identity is the key that aligns across directories: `interlude1` in the
/// draft tree and `interlude-1-ground-level-explorer.md` in the outline tree are
/// the same unit — same identity, same time, same scope.
pub fn identity_of_unit_name(name: &str) -> Option<String> {
    parse_unit_name(name).map(|(identity, _)| identity)
}

/// Take the digits and the letter right after them from a prefix
/// (`5a-father` → `("5", "a")`).
fn split_number(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim_start_matches(['-', '_']);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let letter: String = rest[digits.len()..]
        .chars()
        .take_while(|character| character.is_ascii_lowercase())
        .collect();
    Some((digits, letter))
}

/// Chapter number (`ch07.md` → 7).
pub fn chapter_beat(file_name: &str) -> Option<u32> {
    let stem = file_name.strip_suffix(".md").unwrap_or(file_name);
    let rest = stem.strip_prefix("ch")?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// The layer a fact belongs to (setting / fact / relation), decided by the
/// prefix of its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Setting layer: concept cards.
    Setting,
    /// Fact layer: assertable content in drafts and plans.
    Fact,
    /// Relation layer: relations between entities and their evolution.
    Relation,
}

impl Layer {
    /// Fact-id prefix → layer.
    pub fn of_fact_id(fact_id: &str) -> Layer {
        if fact_id.starts_with("card:") {
            Layer::Setting
        } else if fact_id.starts_with("rel:") {
            Layer::Relation
        } else {
            Layer::Fact
        }
    }

    /// The layer's label.
    ///
    /// These strings are **stored data**, not display chrome: they become the
    /// fact's `kind`, so changing them would make every fact written by an
    /// earlier version unrecognisable. They stay as they are.
    pub fn label(&self) -> &'static str {
        match self {
            Layer::Setting => "设定",
            Layer::Fact => "事实",
            Layer::Relation => "关系",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
