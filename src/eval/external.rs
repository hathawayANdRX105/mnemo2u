//! External benchmark slices → [`Case`] (stage doc 06 T2/T3).
//!
//! The self-built sets in [`super::cases`] are literals written for this
//! harness; MTRAG and LongMemEval are not. This module is the adapter between
//! the two published shapes and the [`Case`] contract, and it is deliberately
//! thin: it maps, it never scores and it never repairs.
//!
//! - **MTRAG** ([`MtragSlice`]) — one accepted conversation of the
//!   `mtrag-human` split. The dataset's own unit is a flat list of messages
//!   with unix timestamps, and the retrieval task adds a question plus qrels
//!   that point at corpus passages. This harness ingests the *conversation*,
//!   never the passages (ClapNQ/FiQA text is third-party corpus and is not
//!   shipped), so the labelled gold evidence is the reviewed agent reply that
//!   answers the task question and the near miss is the sibling reply in the
//!   same conversation.
//! - **LongMemEval** ([`LongmemEvalSlice`]) — one oracle-split instance. Its
//!   unit is a list of sessions, each with a date and a list of
//!   `role`/`content` turns, and its gold label is `has_answer` on the evidence
//!   turns plus the instance answer. Sessions are flattened into one
//!   conversation with session-qualified turn ids (`s{session}t{turn}`), so a
//!   fact id still says which session it came from.
//!
//! ## The query is a projection of the benchmark question
//!
//! The lexical arm is AND-of-tokens ([`crate::store::turso_lexical`]: every
//! whitespace token is quoted and the phrases are joined with `AND`), and this
//! harness runs with the vector arm off, so a natural-language question
//! matches *nothing* — "Do the Arizona Cardinals play outside the US?" carries
//! `us`, which the answer does not. [`probe_terms`] therefore restricts the
//! question to its content words that the labelled gold evidence actually
//! contains, in question order. Nothing is hand-written: the query is derived
//! from the dataset's own question, and every term is a term the gold turn
//! has, so AND-matching is guaranteed to hit it.
//!
//! That projection is also what makes the negative load-bearing. A near-miss
//! turn shares a **strict subset** of the probe's tokens — enough to look
//! relevant, not enough to survive AND — so the `forbidden` marker can only be
//! missed if the matching semantics regress to OR. Both adapters refuse a slice
//! where that property does not hold.
//!
//! ## What this is not
//!
//! No NDCG/MRR over qrels (the criteria are recall + forbidden), no model in
//! the loop, no dataset download: the fixtures under `tests/fixtures` are
//! small labelled slices with their provenance and licence in the file.
//!
//! [`Case`]: super::Case

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{snapshot_id, Case, CaseQuery, Conditions, Turn};
use crate::core::types::ScopeKey;

/// Fixture file name of the MTRAG slice, relative to [`fixture_dir`].
pub const MTRAG_FIXTURE: &str = "mtrag_subset.json";
/// Fixture file name of the LongMemEval slice, relative to [`fixture_dir`].
pub const LONGMEMEVAL_FIXTURE: &str = "longmemeval_subset.json";

/// The character budget `crate::query::facts` cuts a candidate's text to
/// before it reaches the caller. `expected`/`forbidden` are asserted against
/// that summary, not against the stored text, so a marker past the cut could
/// never be found; both adapters refuse such a slice instead of shipping a
/// case that can only fail. Mirrors `crate::query::facts::SUMMARY_CHARS`,
/// which is private to that module.
const L0_SUMMARY_CHARS: usize = 200;

/// Question words that carry no retrieval signal. Kept small and explicit: the
/// probe is the question minus these, so the list only has to cover words that
/// appear verbatim in the labelled evidence of the shipped slices.
const STOP_WORDS: &[&str] = &[
    "about", "after", "again", "all", "also", "and", "any", "are", "been", "before", "being",
    "but", "can", "could", "did", "does", "doing", "each", "for", "from", "get", "got", "had",
    "has", "have", "having", "her", "him", "his", "how", "into", "its", "just", "let", "like",
    "make", "makes", "many", "may", "might", "more", "most", "much", "must", "not", "now", "off",
    "one", "only", "onto", "our", "out", "over", "own", "per", "same", "shall", "she", "should",
    "since", "some", "such", "take", "than", "that", "the", "their", "them", "then", "there",
    "these", "they", "this", "those", "through", "too", "under", "until", "use", "very", "was",
    "way", "were", "what", "when", "where", "which", "while", "who", "whom", "whose", "why",
    "will", "with", "would", "you", "your",
];

/// Where a fixture slice came from, and under what terms it may be used.
///
/// This is a required field: a slice without a source and a licence is
/// refused by both adapters, so the provenance header cannot rot into a
/// comment nobody reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Human-readable name of the dataset and split.
    pub dataset: String,
    /// URL the slice was taken from.
    pub source: String,
    /// Commit (or other revision) the slice was read at.
    pub revision: String,
    /// SPDX-style licence identifier.
    pub license: String,
    /// Copyright line that has to travel with a redistributed slice.
    #[serde(default)]
    pub copyright: Option<String>,
    /// Bibtex of the paper the dataset asks to be cited.
    #[serde(default)]
    pub citation: Option<String>,
    /// What exactly was taken, and what was deliberately left out.
    pub slice: String,
}

/// One MTRAG conversation message (`mtrag-human/conversations/conversations.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MtragMessage {
    /// Slice-local id (`m0`, `m1`, …), the source message index.
    pub turn_id: String,
    /// MTRAG's own speaker label: `user` or `agent`.
    pub speaker: String,
    /// The utterance text, verbatim.
    pub content: String,
    /// Unix seconds, as in the source conversation.
    pub timestamp: i64,
}

/// The MTRAG retrieval task this case replays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MtragQuery {
    /// The released task id (`<conversation>::<question index>`).
    pub task_id: String,
    /// The task question text, verbatim.
    pub question: String,
    /// The message the annotators' answer for the task lives in.
    pub gold_turn_id: String,
    /// Markers that must appear in the gold message's recalled summary.
    pub expected: Vec<String>,
    /// The sibling message that shares vocabulary with the query without
    /// answering it.
    pub near_miss_turn_id: String,
    /// Markers that must not appear in any recalled summary.
    pub forbidden: Vec<String>,
}

/// One accepted MTRAG conversation plus the retrieval task scored against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MtragSlice {
    pub provenance: Provenance,
    /// Stable case id; two runs of the same slice produce the same snapshot.
    pub case_id: String,
    /// Conversation the messages are written and recalled under.
    pub conversation_id: String,
    pub turns: Vec<MtragMessage>,
    pub query: MtragQuery,
}

/// One LongMemEval session turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// `user` or `assistant`, as in the dataset.
    pub role: String,
    /// The message content, verbatim.
    pub content: String,
    /// LongMemEval's `has_answer` label: this turn carries the evidence.
    pub evidence: bool,
}

/// One LongMemEval history session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySession {
    /// The dataset's session id (`answer_35d6c0be_1`).
    pub session_id: String,
    /// Session stamp as released: `2023/05/26 (Fri) 01:49`.
    pub date: String,
    pub turns: Vec<SessionMessage>,
}

/// A `(session, turn)` address inside a LongMemEval slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub session_id: String,
    /// Index into the session's turn list.
    pub turn_index: usize,
}

/// The LongMemEval question this case replays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongmemEvalQuery {
    /// The dataset's instance id.
    pub question_id: String,
    /// The dataset's ability label (`knowledge-update`, …).
    pub question_type: String,
    /// The question text, verbatim.
    pub question: String,
    /// The dataset's gold answer, kept for provenance.
    pub answer: String,
    /// Where the evidence turns are.
    pub gold: EvidenceRef,
    /// Markers that must appear in the evidence turn's recalled summary.
    pub expected: Vec<String>,
    /// The turn that shares vocabulary with the query without answering it.
    pub near_miss: EvidenceRef,
    /// Markers that must not appear in any recalled summary.
    pub forbidden: Vec<String>,
}

/// One LongMemEval instance: its history sessions plus its question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongmemEvalSlice {
    pub provenance: Provenance,
    pub case_id: String,
    pub conversation_id: String,
    pub sessions: Vec<HistorySession>,
    pub query: LongmemEvalQuery,
}

/// Both external cases at `scale`, read from `fixtures`
/// ([`fixture_dir`] in a checkout).
///
/// Fails with the first slice's error; an external case set that silently
/// shrinks to one is worse than a red gate.
pub fn external_all(scale: usize, fixtures: &Path) -> Result<Vec<Case>, String> {
    Ok(vec![
        load_mtrag(&fixtures.join(MTRAG_FIXTURE), scale)?,
        load_longmemeval(&fixtures.join(LONGMEMEVAL_FIXTURE), scale)?,
    ])
}

/// The committed fixture directory of this checkout (`tests/fixtures`).
pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Read `path` as an MTRAG slice and build its case at `scale`.
///
/// `scale` is the distractor-session count the case is frozen at, and it is an
/// input rather than an afterthought because `snapshot_id` hashes it: the same
/// slice at two scales is two cases.
///
/// Errors: the file is missing or is not a slice (`parse` error), or
/// [`mtrag_case`] rejects it.
pub fn load_mtrag(path: &Path, scale: usize) -> Result<Case, String> {
    mtrag_case(&read_slice(path)?, scale)
}

/// Read `path` as a LongMemEval slice and build its case at `scale`.
///
/// Errors: the file is missing or is not a slice (`parse` error), or
/// [`longmemeval_case`] rejects it.
pub fn load_longmemeval(path: &Path, scale: usize) -> Result<Case, String> {
    longmemeval_case(&read_slice(path)?, scale)
}

/// Map an MTRAG slice onto the [`Case`] contract.
///
/// `user` becomes a `user` turn and MTRAG's `agent` an `assistant` turn, the
/// conversation keeps its own message order and timestamps, and the task
/// question becomes the case's single query with the AND-of-tokens probe of
/// [`probe_terms`].
///
/// Errors (never a panic): empty corpus, missing or duplicated turn ids, a
/// question that projects to fewer than two probe terms, a marker that is not
/// inside the labelled turn's summary window, a `forbidden` marker that also
/// sits in the gold window (a case that could never pass), a near-miss that
/// does not share a strict subset of the probe (a `forbidden` marker that
/// could never fail), missing provenance, or an unknown speaker label.
pub fn mtrag_case(slice: &MtragSlice, scale: usize) -> Result<Case, String> {
    check_provenance(&slice.provenance, "mtrag")?;
    if slice.query.task_id.trim().is_empty() {
        return Err(format!(
            "mtrag slice {}: the released task id is empty",
            slice.case_id
        ));
    }
    if slice.turns.is_empty() {
        return Err(format!("mtrag slice {}: no turns", slice.case_id));
    }
    let mut turns = Vec::with_capacity(slice.turns.len());
    for message in &slice.turns {
        turns.push(Turn {
            turn_id: message.turn_id.clone(),
            role: role_for_mtrag(&message.speaker)?,
            content: message.content.clone(),
            timestamp: message.timestamp,
        });
    }
    let gold = turn_content(&turns, &slice.query.gold_turn_id, "mtrag gold")?;
    let near_miss = turn_content(&turns, &slice.query.near_miss_turn_id, "mtrag near-miss")?;
    assemble(
        Normalized {
            label: "mtrag",
            case_id: slice.case_id.clone(),
            set: "mtrag",
            conversation_id: slice.conversation_id.clone(),
            turns,
            question: slice.query.question.clone(),
            expected: slice.query.expected.clone(),
            forbidden: slice.query.forbidden.clone(),
            // The self-built sets' starting point: recall has to be total,
            // wrong evidence is a failure, and rank is not asserted because a
            // question turn shares the probe and can rank first.
            conditions: Conditions {
                top_k: 5,
                min_recall: 1.0,
                forbid_wrong_evidence: true,
                expect_top_hit: false,
                // No headroom declared: this set is measured against perfect
                // recall, and it reaches it.
                target_recall: 1.0,
            },
            // The latency sample size, nothing else.
            repetitions: 3,
        },
        &gold,
        &near_miss,
        scale,
    )
}

/// Map a LongMemEval slice onto the [`Case`] contract.
///
/// The sessions are flattened into one conversation in session order, with
/// `s{session}t{turn}` turn ids and the session stamp as the turn timestamp,
/// so the multi-session structure survives into the fact ids. The question
/// becomes the case's single query with the AND-of-tokens probe of
/// [`probe_terms`].
///
/// Errors: the same set as [`mtrag_case`], plus a dataset label that is
/// missing, a session stamp that is not `YYYY/MM/DD (Day) HH:MM`, an empty
/// session, a `(session, turn)` address that does not resolve, an address that
/// does not land on a turn the dataset labels as evidence, or a role that is
/// not `user`/`assistant`.
pub fn longmemeval_case(slice: &LongmemEvalSlice, scale: usize) -> Result<Case, String> {
    check_provenance(&slice.provenance, "longmemeval")?;
    for (field, value) in [
        ("question_id", &slice.query.question_id),
        ("question_type", &slice.query.question_type),
        ("answer", &slice.query.answer),
    ] {
        if value.trim().is_empty() {
            return Err(format!(
                "longmemeval slice {}: the dataset label {field} is empty",
                slice.case_id
            ));
        }
    }

    if slice.sessions.is_empty() {
        return Err(format!("longmemeval slice {}: no sessions", slice.case_id));
    }

    let mut turns = Vec::new();
    for (session_index, session) in slice.sessions.iter().enumerate() {
        if session.turns.is_empty() {
            return Err(format!(
                "longmemeval slice {}: session {} has no turns",
                slice.case_id, session.session_id
            ));
        }
        // The session stamp is the only timestamp the dataset gives for a
        // turn, so every turn of a session shares it.
        let timestamp = parse_session_date(&session.date).map_err(|error| {
            format!(
                "longmemeval slice {}: session {}: {error}",
                slice.case_id, session.session_id
            )
        })?;
        for (turn_index, message) in session.turns.iter().enumerate() {
            turns.push(Turn {
                turn_id: format!("s{session_index}t{turn_index}"),
                role: role_for_longmemeval(&message.role)?,
                content: message.content.clone(),
                timestamp,
            });
        }
    }
    let gold_turn = addressed_message(slice, &slice.query.gold, "longmemeval gold")?;
    if !gold_turn.evidence {
        return Err(format!(
            "longmemeval slice {}: the gold address does not point at a turn the dataset \
             labels as evidence",
            slice.case_id
        ));
    }
    let near_miss_turn = addressed_message(slice, &slice.query.near_miss, "longmemeval near-miss")?;
    assemble(
        Normalized {
            label: "longmemeval",
            case_id: slice.case_id.clone(),
            set: "longmemeval",
            conversation_id: slice.conversation_id.clone(),
            turns,
            question: slice.query.question.clone(),
            expected: slice.query.expected.clone(),
            forbidden: slice.query.forbidden.clone(),
            conditions: Conditions {
                top_k: 5,
                min_recall: 1.0,
                forbid_wrong_evidence: true,
                expect_top_hit: false,
                // No headroom declared: this set is measured against perfect
                // recall, and it reaches it.
                target_recall: 1.0,
            },
            repetitions: 2,
        },
        &gold_turn.content,
        &near_miss_turn.content,
        scale,
    )
}

/// A dataset normalized to the shape both adapters finish from.
struct Normalized {
    label: &'static str,
    case_id: String,
    set: &'static str,
    conversation_id: String,
    turns: Vec<Turn>,
    question: String,
    expected: Vec<String>,
    forbidden: Vec<String>,
    conditions: Conditions,
    repetitions: usize,
}

/// Validate the slice and freeze it into a [`Case`].
///
/// Shared by both adapters: everything dataset-specific has already been
/// mapped by the time this runs, so the frozen criteria, the probe and the
/// snapshot id are computed exactly once, in one place.
fn assemble(
    normalized: Normalized,
    gold: &str,
    near_miss: &str,
    scale: usize,
) -> Result<Case, String> {
    let Normalized {
        label,
        case_id,
        set,
        conversation_id,
        turns,
        question,
        expected,
        forbidden,
        conditions,
        repetitions,
    } = normalized;
    let fail = |reason: String| format!("{label} slice {case_id}: {reason}");

    if case_id.trim().is_empty() {
        return Err(fail("case_id is empty".into()));
    }
    if conversation_id.trim().is_empty() {
        return Err(fail("conversation_id is empty".into()));
    }
    if turns.is_empty() {
        return Err(fail("no turns".into()));
    }
    if question.trim().is_empty() {
        return Err(fail("no question".into()));
    }
    let mut seen = BTreeSet::new();
    for turn in &turns {
        if turn.content.trim().is_empty() {
            return Err(fail(format!("turn {} has no content", turn.turn_id)));
        }
        if turn.turn_id.is_empty() {
            return Err(fail("a turn has no id".into()));
        }
        if !seen.insert(turn.turn_id.clone()) {
            return Err(fail(format!("duplicate turn id {}", turn.turn_id)));
        }
    }

    // Markers are matched against the L0 summary, so a marker past the cut is
    // a case that can never pass.
    if expected.is_empty() {
        return Err(fail("no expected marker".into()));
    }
    for marker in &expected {
        if !summary(gold).contains(marker.as_str()) {
            return Err(fail(format!(
                "expected marker {marker:?} is not inside the gold turn's {}-character summary",
                L0_SUMMARY_CHARS
            )));
        }
    }
    if forbidden.is_empty() {
        return Err(fail("no forbidden marker".into()));
    }
    for marker in &forbidden {
        if !summary(near_miss).contains(marker.as_str()) {
            return Err(fail(format!(
                "forbidden marker {marker:?} is not inside the near-miss turn's \
                 {L0_SUMMARY_CHARS}-character summary"
            )));
        }
        // The gold turn is recalled for this query, so a forbidden marker that
        // also sits in it fails every run.
        if summary(gold).contains(marker.as_str()) {
            return Err(fail(format!(
                "forbidden marker {marker:?} is also in the gold turn's summary"
            )));
        }
    }

    let probe = probe_terms(&question, gold);
    if probe.len() < 2 {
        return Err(fail(format!(
            "the question projects onto {} probe terms ({probe:?}); the lexical arm needs at \
             least two for AND-of-tokens to be meaningful",
            probe.len()
        )));
    }
    // The negative is only load-bearing if AND-matching can exclude the
    // near-miss: it must share some probe terms (so it looks relevant) but not
    // all of them (so only OR matching would surface it).
    let near_miss_terms: BTreeSet<String> = tokens(near_miss).into_iter().collect();
    let shared = probe
        .iter()
        .filter(|term| near_miss_terms.contains(*term))
        .count();
    if shared == 0 || shared == probe.len() {
        return Err(fail(format!(
            "the near-miss shares {shared} of the {} probe terms ({probe:?}); a forbidden marker \
             needs a strict non-empty subset to be able to fail",
            probe.len()
        )));
    }

    let queries = vec![CaseQuery {
        text: probe.join(" "),
        expected,
        forbidden,
    }];
    let snapshot_id = snapshot_id(&case_id, &turns, &queries, scale, &conditions);
    Ok(Case {
        case_id,
        set: set.to_string(),
        snapshot_id,
        scope: ScopeKey {
            workspace_id: "eval-ws".into(),
            session_id: None,
            task_id: None,
        },
        conversation_id,
        turns,
        distractor_sessions: scale,
        queries,
        conditions,
        repetitions,
    })
}

/// The recall probe for one benchmark question.
///
/// The lexical arm is AND-of-tokens and the vector arm is off in this
/// harness, so a prose question matches nothing; this restricts it to the
/// question's content words the labelled gold evidence actually contains, in
/// question order. Every term is a term the gold turn has, so the probe always
/// AND-matches it.
///
/// Only ASCII words of at least three characters are kept: a token carrying a
/// digit or an apostrophe would be split differently by the FTS analyzer than
/// by [`tokens`], and the projection must not depend on that.
fn probe_terms(question: &str, evidence: &str) -> Vec<String> {
    let evidence_terms: BTreeSet<String> = tokens(evidence).into_iter().collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut probe = Vec::new();
    for term in tokens(question) {
        let usable = term.len() >= 3
            && term.chars().all(|ch| ch.is_ascii_alphabetic())
            && !STOP_WORDS.contains(&term.as_str())
            && evidence_terms.contains(&term);
        if usable && seen.insert(term.clone()) {
            probe.push(term);
        }
    }
    probe
}

/// Lowercased alphanumeric runs, the same shape a whitespace-split FTS query
/// tokenizes to once punctuation is dropped.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The part of a candidate text the harness actually asserts markers against.
fn summary(text: &str) -> &str {
    let end = text
        .char_indices()
        .nth(L0_SUMMARY_CHARS)
        .map_or(text.len(), |(offset, _)| offset);
    &text[..end]
}

fn read_slice<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", path.display()))
}

fn check_provenance(provenance: &Provenance, label: &str) -> Result<(), String> {
    if provenance.source.trim().is_empty() {
        return Err(format!("{label} slice: provenance.source is empty"));
    }
    if provenance.license.trim().is_empty() {
        return Err(format!("{label} slice: provenance.license is empty"));
    }
    if provenance.revision.trim().is_empty() {
        return Err(format!("{label} slice: provenance.revision is empty"));
    }
    Ok(())
}

fn role_for_mtrag(speaker: &str) -> Result<String, String> {
    match speaker {
        "user" => Ok("user".into()),
        "agent" => Ok("assistant".into()),
        other => Err(format!("mtrag slice: unknown speaker {other:?}")),
    }
}

fn role_for_longmemeval(role: &str) -> Result<String, String> {
    match role {
        "user" | "assistant" => Ok(role.to_string()),
        other => Err(format!("longmemeval slice: unknown role {other:?}")),
    }
}

/// The text of one turn, addressed by turn id. Owned so the caller can hand
/// `turns` to the case builder afterwards.
fn turn_content(turns: &[Turn], turn_id: &str, what: &str) -> Result<String, String> {
    let mut found: Option<&Turn> = None;
    for turn in turns {
        if turn.turn_id == turn_id {
            if found.is_some() {
                return Err(format!("{what}: turn id {turn_id:?} is ambiguous"));
            }
            found = Some(turn);
        }
    }
    found
        .map(|turn| turn.content.clone())
        .ok_or_else(|| format!("{what}: no turn with id {turn_id:?}"))
}

/// Resolve a `(session, turn_index)` address to the message it names.
///
/// The address is resolved through the slice's own session list, so an
/// out-of-range index names the session it was meant for instead of silently
/// landing on a turn of another session.
fn addressed_message<'a>(
    slice: &'a LongmemEvalSlice,
    reference: &EvidenceRef,
    what: &str,
) -> Result<&'a SessionMessage, String> {
    let session = slice
        .sessions
        .iter()
        .find(|session| session.session_id == reference.session_id)
        .ok_or_else(|| {
            format!(
                "{what}: slice {} has no session {:?}",
                slice.case_id, reference.session_id
            )
        })?;
    session.turns.get(reference.turn_index).ok_or_else(|| {
        format!(
            "{what}: session {:?} has {} turns, index {} is out of range",
            reference.session_id,
            session.turns.len(),
            reference.turn_index
        )
    })
}

/// Parse a LongMemEval session stamp (`2023/05/26 (Fri) 01:49`) to unix
/// seconds, read as UTC.
///
/// The stamp is the dataset's own simulated clock; what recall needs from it
/// is the ordering, and neither the temporal arm (which centres on a date
/// parsed out of the query) nor the bi-temporal rows depend on the zone.
fn parse_session_date(text: &str) -> Result<i64, String> {
    let mut parts = text.split_whitespace();
    let (date, weekday, time) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(date), Some(weekday), Some(time), None) => (date, weekday, time),
        _ => {
            return Err(format!(
                "session date {text:?} is not `YYYY/MM/DD (Day) HH:MM`"
            ))
        }
    };
    if !weekday.starts_with('(') || !weekday.ends_with(')') {
        return Err(format!("session date {text:?} has no (Day) field"));
    }
    let mut date_parts = date.split('/');
    let (year, month, day) = match (
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
    ) {
        (Some(year), Some(month), Some(day), None) => (year, month, day),
        _ => return Err(format!("session date {text:?} has no `YYYY/MM/DD`")),
    };
    let mut time_parts = time.split(':');
    let (hour, minute) = match (time_parts.next(), time_parts.next(), time_parts.next()) {
        (Some(hour), Some(minute), None) => (hour, minute),
        _ => return Err(format!("session date {text:?} has no `HH:MM`")),
    };
    let number = |field: &str, text: &str| -> Result<i64, String> {
        text.parse::<i64>()
            .map_err(|_| format!("session date {text:?}: {field} {text:?} is not a number"))
    };
    let (year, month, day) = (
        number("year", year)?,
        number("month", month)?,
        number("day", day)?,
    );
    let (hour, minute) = (number("hour", hour)?, number("minute", minute)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(format!("session date {text:?}: month or day out of range"));
    }
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return Err(format!("session date {text:?}: time out of range"));
    }
    Ok(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm), the
/// same arithmetic `super::stamp` uses to format a UTC stamp.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = if shifted >= 0 { shifted } else { shifted - 399 } / 400;
    let year_of_era = shifted - era * 400;
    let month_shift = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}
