//! Cost gates — chunk value scoring and the call ledger (`src/index/cost.rs`).

use mnemo2u::index::cost::{ChunkValueScorer, CostLedger, CostOptions, CostStage};

fn options() -> CostOptions {
    CostOptions {
        min_chunk_tokens: 10,
        duplicate_similarity: 0.8,
        max_scored_chunks: 3,
    }
}

/// 20 words; the duplicate differs only in the final one, so 15 of 17 shingles
/// still match (0.88, over the 0.8 threshold).
const NEAR_DUPLICATE_A: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango uniform";
const NEAR_DUPLICATE_B: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango VICTOR";

#[test]
fn short_chunk_is_never_extracted() {
    let mut scorer = ChunkValueScorer::new();
    let verdict = scorer.score("tiny", 2, &options());
    assert_eq!(verdict, mnemo2u::index::cost::ChunkVerdict::TooShort);
    assert_eq!(scorer.scored(), 0, "no LLM call is spent on it");
}

#[test]
fn distinct_chunk_is_kept() {
    let mut scorer = ChunkValueScorer::new();
    let verdict = scorer.score("ACME acquired Beta Labs in a deal", 30, &options());
    assert_eq!(verdict, mnemo2u::index::cost::ChunkVerdict::Keep);
    assert_eq!(scorer.scored(), 1);
}

#[test]
fn near_duplicate_chunk_is_skipped() {
    let mut scorer = ChunkValueScorer::new();
    assert_eq!(
        scorer.score(NEAR_DUPLICATE_A, 30, &options()),
        mnemo2u::index::cost::ChunkVerdict::Keep
    );
    assert_eq!(
        scorer.score(NEAR_DUPLICATE_B, 30, &options()),
        mnemo2u::index::cost::ChunkVerdict::Duplicate,
        "a re-sent chunk must not cost a second extraction"
    );
}

#[test]
fn dissimilar_long_chunk_is_kept() {
    let mut scorer = ChunkValueScorer::new();
    assert_eq!(
        scorer.score(NEAR_DUPLICATE_A, 30, &options()),
        mnemo2u::index::cost::ChunkVerdict::Keep
    );
    assert_eq!(
        scorer.score("zulu yankee xray whiskey victor sierra tango romeo quebec papa oscar november mike lima kilo juliet india hotel golf foxtrot echo delta charlie bravo alpha", 30, &options()),
        mnemo2u::index::cost::ChunkVerdict::Keep,
        "a reversed document shares no 5-gram shingles"
    );
    assert_eq!(scorer.scored(), 2);
}

#[test]
fn per_run_cap_bounds_the_spend() {
    let mut scorer = ChunkValueScorer::new();
    for index in 0..4 {
        let text = format!("distinct document number {index} with plenty of words to score");
        let verdict = scorer.score(&text, 30, &options());
        if index < 3 {
            assert_eq!(verdict, mnemo2u::index::cost::ChunkVerdict::Keep);
        } else {
            assert_eq!(verdict, mnemo2u::index::cost::ChunkVerdict::OverBudget);
        }
    }
    assert_eq!(scorer.scored(), 3, "the cap is exact");
}

#[test]
fn ledger_counts_calls_cache_hits_and_tokens_per_stage() {
    let ledger = CostLedger::new();
    ledger.record(CostStage::Extract, "mock", false, 100);
    ledger.record(CostStage::Extract, "mock", false, 50);
    ledger.record(CostStage::Extract, "mock", true, 100);
    ledger.record(CostStage::Keywords, "mock", false, 10);

    let rows = ledger.rows();
    let extract = rows
        .get(&(CostStage::Extract, "mock".to_string()))
        .expect("extract row")
        .clone();
    assert_eq!(extract.calls, 2, "cache hits are not fresh calls");
    assert_eq!(extract.cache_hits, 1);
    assert_eq!(extract.input_tokens, 250, "every call's tokens are counted");

    let keywords = rows
        .iter()
        .find(|((stage, _), _)| *stage == CostStage::Keywords)
        .map(|(_, row)| (*row).clone())
        .expect("keywords row");
    assert_eq!(keywords.calls, 1);
    assert_eq!(keywords.cache_hits, 0);
}

#[test]
fn ledger_tracks_skipped_chunks() {
    let ledger = CostLedger::new();
    ledger.note_scored_chunk();
    ledger.note_scored_chunk();
    ledger.note_skipped_chunk();
    assert_eq!(ledger.scored_chunks(), 2);
    assert_eq!(ledger.skipped_chunks(), 1);
}
