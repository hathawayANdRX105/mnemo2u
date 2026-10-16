//! The external evaluation sets (stage doc 06 T2/T3): the MTRAG and
//! LongMemEval slices reach the same runner the self-built cases do, so a
//! regression in the adapters is red on the same criteria.
//!
//! Two properties are asserted beyond "the run passes", because both are the
//! ones an adapter can silently lose:
//!
//! - the **gold** label survives the mapping — the marker the dataset's own
//!   qrels / `has_answer` labels point at has to come back in the top-k;
//! - the **near miss** is still able to fail — the `forbidden` marker is
//!   asserted by re-running the case with a probe the near-miss also matches,
//!   so a regression to OR matching shows up as a red case rather than as a
//!   negative assertion that can never go off.

use std::path::{Path, PathBuf};

use mnemo2u::eval::external::{
    self, LongmemEvalSlice, MtragSlice, LONGMEMEVAL_FIXTURE, MTRAG_FIXTURE,
};
use mnemo2u::eval::{run_case, snapshot_id, Case, Failure};
use serde::Serialize;

/// Run one case in a fresh workspace.
async fn run(case: &Case, name: &str) -> mnemo2u::eval::CaseResult {
    let dir = std::env::temp_dir().join(format!("mnemo2u-eval-external-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    run_case(case, &dir).await.expect("case runs")
}

fn fixture(name: &str) -> PathBuf {
    external::fixture_dir().join(name)
}

/// The committed slices, parsed — the round-trip and rejection tests mutate
/// these in memory and write the mutation out to a scratch file.
fn read_mtrag() -> MtragSlice {
    let raw = std::fs::read_to_string(fixture(MTRAG_FIXTURE)).expect("mtrag fixture is committed");
    serde_json::from_str(&raw).expect("mtrag fixture parses")
}

fn read_longmemeval() -> LongmemEvalSlice {
    let raw = std::fs::read_to_string(fixture(LONGMEMEVAL_FIXTURE))
        .expect("longmemeval fixture is committed");
    serde_json::from_str(&raw).expect("longmemeval fixture parses")
}

fn write_slice<T: Serialize>(dir: &Path, name: &str, slice: &T) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_string(slice).expect("slice serializes"),
    )
    .expect("scratch fixture is written");
    path
}

/// Lowercased alphanumeric runs, computed independently of the adapter so the
/// tests do not assert the adapter's own tokenizer against itself.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The MTRAG slice replays one retrieval task over a whole conversation: the
/// gold marker the annotators' answer carries has to be recalled out of the
/// 16-message corpus, and the sibling Cardinals-team answer must not answer in
/// its place.
#[tokio::test]
async fn mtrag_case_recalls_gold_evidence() {
    let case = external::load_mtrag(&fixture(MTRAG_FIXTURE), 0).expect("mtrag slice loads");
    assert_eq!(case.set, "mtrag");
    assert_eq!(case.turns.len(), 16, "the whole conversation is the corpus");

    let result = run(&case, "mtrag").await;
    assert!(
        result.passed(),
        "mtrag failed: {result:#?}\nmanifest: {}",
        mnemo2u::eval::Report::manifest_json(&case, &result).expect("manifest")
    );
    assert_eq!(result.quality.recall_at_k, 1.0);
    assert_eq!(result.quality.wrong_evidence_rate, 0.0);
    assert_eq!(
        result.queries.len(),
        1,
        "one retrieval task per slice: {:?}",
        result.queries
    );
    assert!(
        result.queries[0].missing.is_empty(),
        "the gold marker was not recalled: {:#?}",
        result.queries[0]
    );
    assert!(
        !result.queries[0].returned_ids.is_empty(),
        "recall returned nothing for {query:?}",
        query = result.queries[0].text
    );
}

/// The LongMemEval slice is a knowledge-update instance: the question asks
/// about the *previous* tutor Juan, whose Wednesday class carries the gold
/// label, while the session where the user swapped to a different tutor is the
/// near miss that must not answer it.
///
/// The second half is the load-bearing one: dropping the probe terms only the
/// gold turn carries lets the near miss into the top-k, and the frozen
/// `forbidden` marker has to fail the run. If the forbidden markers were
/// decorative — dropped by the adapter, or pointing at nothing — this goes
/// green while the negative is dead.
#[tokio::test]
async fn longmemeval_case_flags_wrong_evidence() {
    let case =
        external::load_longmemeval(&fixture(LONGMEMEVAL_FIXTURE), 0).expect("lme slice loads");
    assert_eq!(case.set, "longmemeval");

    let result = run(&case, "longmemeval").await;
    assert!(
        result.passed(),
        "longmemeval failed: {result:#?}\nmanifest: {}",
        mnemo2u::eval::Report::manifest_json(&case, &result).expect("manifest")
    );
    assert_eq!(result.quality.recall_at_k, 1.0);
    assert_eq!(result.quality.wrong_evidence_rate, 0.0);

    let marker = case.queries[0]
        .forbidden
        .first()
        .expect("the slice labels a near miss")
        .clone();
    let near_miss = case
        .turns
        .iter()
        .find(|turn| turn.content.contains(&marker))
        .expect("the forbidden marker names a turn of the corpus");
    let near_miss_words = words(&near_miss.content);
    let frozen: Vec<String> = case.queries[0]
        .text
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let shared: Vec<String> = frozen
        .iter()
        .filter(|term| near_miss_words.contains(*term))
        .cloned()
        .collect();
    assert!(
        !shared.is_empty() && shared.len() < frozen.len(),
        "the near miss must share a strict subset of the probe \
         (probe {frozen:?}, shared {shared:?}) or the forbidden marker cannot fail"
    );

    // The permissive probe is a different case, so its snapshot rotates.
    let mut permissive = case.clone();
    permissive.queries[0].text = shared.join(" ");
    permissive.snapshot_id = snapshot_id(
        &permissive.case_id,
        &permissive.turns,
        &permissive.queries,
        permissive.distractor_sessions,
        &permissive.conditions,
    );

    let outcome = run(&permissive, "longmemeval-permissive").await;
    assert!(
        !outcome.passed(),
        "a probe the near miss also matches must fail the frozen criteria: {outcome:#?}"
    );
    assert!(
        outcome
            .queries
            .iter()
            .any(|query| query.wrong_evidence.contains(&marker)),
        "the near miss surfaced but was not reported as wrong evidence: {:#?}",
        outcome.queries
    );
    assert!(
        outcome.failures.iter().any(|failure| matches!(
            failure,
            Failure::WrongEvidence { found, .. } if found.contains(&marker)
        )),
        "expected a WrongEvidence failure naming {marker:?}: {:#?}",
        outcome.failures
    );
}

/// The fixtures rebuild a complete case, and the snapshot id is the audit
/// handle: loading twice gives the same case, a corpus edit rotates it, and
/// so does a scale edit.
#[test]
fn external_fixtures_round_trip() {
    let first = external::load_mtrag(&fixture(MTRAG_FIXTURE), 0).expect("mtrag slice loads");
    let again = external::load_mtrag(&fixture(MTRAG_FIXTURE), 0).expect("mtrag slice loads");
    assert_eq!(first, again, "the same fixture must build the same case");
    assert_eq!(first.queries.len(), 1);
    assert!(!first.queries[0].expected.is_empty());
    assert!(!first.queries[0].forbidden.is_empty());
    assert!(!first.queries[0].text.is_empty());

    let mut edited_slice = read_mtrag();
    edited_slice.turns[0]
        .content
        .push_str(" an edited opening turn");
    let edited = external::mtrag_case(&edited_slice, 0).expect("the edited slice still builds");
    assert_ne!(
        first.snapshot_id, edited.snapshot_id,
        "a corpus edit has to rotate the snapshot or a frozen criterion is not frozen"
    );

    let scaled = external::load_mtrag(&fixture(MTRAG_FIXTURE), 3).expect("scaled slice loads");
    assert_ne!(first.snapshot_id, scaled.snapshot_id);
    assert_eq!(scaled.distractor_sessions, 3);

    let lme = external::load_longmemeval(&fixture(LONGMEMEVAL_FIXTURE), 0)
        .expect("longmemeval slice loads");
    let lme_again =
        external::load_longmemeval(&fixture(LONGMEMEVAL_FIXTURE), 0).expect("longmemeval loads");
    assert_eq!(lme, lme_again);
    assert_eq!(lme.turns.len(), 22, "both oracle sessions are the corpus");

    let mut edited_lme = read_longmemeval();
    edited_lme.sessions[0].turns[0]
        .content
        .push_str(" an edited opening turn");
    let edited_lme =
        external::longmemeval_case(&edited_lme, 0).expect("the edited slice still builds");
    assert_ne!(lme.snapshot_id, edited_lme.snapshot_id);

    let both = external::external_all(0, &external::fixture_dir()).expect("both slices load");
    let sets: Vec<&str> = both.iter().map(|case| case.set.as_str()).collect();
    assert_eq!(sets, vec!["mtrag", "longmemeval"]);
}

/// A slice that cannot produce a meaningful case is an error, not a panic and
/// not a silently weaker case: an empty corpus, an unusable question, a marker
/// that no labelled turn carries, a negative that can never fail, and a missing
/// licence all have to be refused.
#[test]
fn empty_or_broken_fixtures_are_rejected() {
    let scratch = tempfile::tempdir().expect("scratch dir");

    let empty = scratch.path().join("empty.json");
    std::fs::write(&empty, "").expect("write");
    assert!(
        external::load_mtrag(&empty, 0).is_err(),
        "an empty file is not a slice"
    );

    let missing = scratch.path().join("does-not-exist.json");
    assert!(
        external::load_mtrag(&missing, 0).is_err(),
        "a missing file is an error"
    );

    let mut no_turns = read_mtrag();
    no_turns.turns.clear();
    assert!(
        external::load_mtrag(&write_slice(scratch.path(), "no-turns.json", &no_turns), 0).is_err(),
        "a slice with no corpus is an error"
    );

    let mut stray_marker = read_mtrag();
    stray_marker.query.expected = vec!["a sentence no turn in the corpus carries".into()];
    assert!(
        external::load_mtrag(
            &write_slice(scratch.path(), "stray-marker.json", &stray_marker),
            0
        )
        .is_err(),
        "an expected marker outside the gold turn can never be recalled"
    );

    let mut self_contradictory = read_mtrag();
    // "Arizona Cardinals" is inside both the gold and the near-miss summary,
    // so this is the self-contradiction rule and nothing else.
    self_contradictory.query.forbidden = vec!["Arizona Cardinals".into()];
    assert!(
        external::load_mtrag(
            &write_slice(
                scratch.path(),
                "self-contradictory.json",
                &self_contradictory
            ),
            0
        )
        .is_err(),
        "a forbidden marker inside the gold turn fails every run"
    );

    let mut unrelated_near_miss = read_mtrag();
    // The unanswerable first exchange: it shares no probe term with the task
    // question, so the forbidden marker could never fail.
    unrelated_near_miss.query.near_miss_turn_id = "m1".into();
    unrelated_near_miss.query.forbidden = vec!["I'm sorry, but I don't have the answer".into()];
    assert!(
        external::load_mtrag(
            &write_slice(
                scratch.path(),
                "unrelated-near-miss.json",
                &unrelated_near_miss
            ),
            0
        )
        .is_err(),
        "a near miss that shares no probe token makes the forbidden marker decorative"
    );

    let mut unlicensed = read_mtrag();
    unlicensed.provenance.license = "  ".into();
    assert!(
        external::load_mtrag(
            &write_slice(scratch.path(), "unlicensed.json", &unlicensed),
            0
        )
        .is_err(),
        "a slice without a licence is not committable"
    );

    let mut unanswerable_question = read_mtrag();
    unanswerable_question.query.question = "the cat sat on the mat".into();
    assert!(
        external::load_mtrag(
            &write_slice(scratch.path(), "unanswerable.json", &unanswerable_question),
            0
        )
        .is_err(),
        "a question that projects onto no probe terms cannot be replayed"
    );

    let mut bad_session = read_longmemeval();
    bad_session.sessions[0].date = "2023/05/26".into();
    assert!(
        external::load_longmemeval(
            &write_slice(scratch.path(), "bad-session.json", &bad_session),
            0
        )
        .is_err(),
        "a session without a full stamp has no turn time"
    );

    let mut dangling_gold = read_longmemeval();
    dangling_gold.query.gold.turn_index = 9_999;
    assert!(
        external::load_longmemeval(
            &write_slice(scratch.path(), "dangling-gold.json", &dangling_gold),
            0
        )
        .is_err(),
        "an evidence address outside the session is an error"
    );
}
