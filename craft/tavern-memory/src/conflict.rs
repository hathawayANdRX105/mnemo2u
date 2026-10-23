//! Conflict detection: an event that contradicts the canon.
//!
//! The tavern's problem is not retrieval — it is that the fiction has to stay
//! consistent with itself. A character sheet says she never smokes; a session
//! has her light a cigarette. One of them is wrong, and **the craft does not
//! get to decide which**: that is a judgement about the fiction, so it goes to
//! the judge.
//!
//! What this module does is narrower and mechanical:
//!
//! 1. find the canon entries an event might touch;
//! 2. compare them;
//! 3. report the collision, with both statements and the event's story time.
//!
//! The comparison is deliberately conservative. A conflict is only reported
//! when the two statements are about the same thing **and** disagree — a
//! negated statement against a plain one, or two statements about the same
//! subject with incompatible values. Anything vaguer is left alone, because a
//! false conflict is worse than a missed one: it sends the judge off to
//! adjudicate a non-problem.

use crate::{Conflict, SessionEvent};

/// A canon entry's subject: the thing the statement is about.
///
/// The subject is derived from the statement's first clause, so "She never
/// smokes" and "She smokes" share a subject ("she") and can be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    /// The subject, lowercased and trimmed.
    pub text: String,
    /// The predicate: what is claimed about the subject.
    pub predicate: String,
    /// Whether the statement is negated ("never", "doesn't", "不").
    pub negated: bool,
}

impl Subject {
    /// Split a statement into subject and predicate.
    ///
    /// The split is the first whitespace or comma: "She never smokes" has the
    /// subject "she" and the predicate "never smokes"; "Lilian, the heir,
    /// refuses" has the subject "lilian".
    #[must_use]
    pub fn of(statement: &str) -> Self {
        let trimmed = statement.trim();
        let separator = trimmed
            .find(|character: char| character.is_whitespace() || character == ',')
            .unwrap_or(trimmed.len());
        let subject = trimmed[..separator].trim().to_lowercase();
        let predicate = trimmed[separator..].trim().to_lowercase();
        let negated = contains_negation(&predicate);
        Self {
            text: subject,
            predicate,
            negated,
        }
    }

    /// Whether two statements are about the same subject.
    #[must_use]
    pub fn same_subject(&self, other: &Subject) -> bool {
        !self.text.is_empty() && self.text == other.text
    }
}

/// Whether a predicate carries a negation.
fn contains_negation(predicate: &str) -> bool {
    const NEGATIONS: [&str; 8] = ["never", "no ", "not ", "n't", "不", "从不", "没有", "无"];
    NEGATIONS.iter().any(|needle| predicate.contains(needle))
}

/// Whether two statements contradict each other.
///
/// Two statements can only contradict each other if they are about the same
/// thing, so the subject check comes first — otherwise "she never smokes"
/// and "he always smokes" would read as a contradiction, which they are not.
///
/// Given the same subject, two ways to disagree, and only two:
///
/// - one is negated and the other is not ("she never smokes" vs "she smokes");
/// - they name different values for the same attribute ("her eyes are blue"
///   vs "her eyes are green").
#[must_use]
pub fn disagrees(left: &Subject, right: &Subject) -> bool {
    if !left.same_subject(right) {
        return false;
    }
    if left.negated != right.negated {
        return true;
    }
    let left_value = value_of(&left.predicate);
    let right_value = value_of(&right.predicate);
    match (left_value, right_value) {
        (Some(left_value), Some(right_value)) => left_value != right_value,
        _ => false,
    }
}

/// The value a predicate claims, when it claims one.
///
/// "her eyes are blue" → "blue"; "she smokes" → nothing, because the
/// predicate is a bare action with no value to contradict.
fn value_of(predicate: &str) -> Option<String> {
    const CONNECTORS: [&str; 4] = [" are ", " is ", " was ", " has "];
    for connector in CONNECTORS {
        if let Some(position) = predicate.find(connector) {
            let value = predicate[position + connector.len()..].trim();
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

/// Find the conflicts between a session's events and the canon.
///
/// The canon entries are compared against every event, so the caller gets the
/// full list of collisions rather than the first one.
#[must_use]
pub fn detect<'a>(
    canon: impl IntoIterator<Item = &'a crate::CanonEntry>,
    events: impl IntoIterator<Item = &'a SessionEvent>,
) -> Vec<Conflict> {
    let canon: Vec<&crate::CanonEntry> = canon.into_iter().collect();
    let mut conflicts = Vec::new();
    for event in events {
        let event_subject = Subject::of(&event.text);
        for entry in &canon {
            let canon_subject = Subject::of(&entry.text);
            if !disagrees(&canon_subject, &event_subject) {
                continue;
            }
            conflicts.push(Conflict {
                canon_key: entry.key.clone(),
                canon_text: entry.text.clone(),
                event_key: event.key.clone(),
                event_text: event.text.clone(),
                tick: event.tick,
            });
        }
    }
    conflicts
}
