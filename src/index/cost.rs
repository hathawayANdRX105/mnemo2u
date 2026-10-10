//! Cost gates — the R2 accounting the stage defers from the ported path.
//!
//! Two mechanisms, both zero-LLM unless explicitly triggered:
//!
//! 1. **Chunk value scoring** (`src/index/cost.rs::ChunkValueScorer`): a
//!    token/length/duplication heuristic that decides whether a chunk is worth
//!    an extraction call at all. Low-value chunks produce no extraction
//!    records, so the LLM is never called for them — the KET-RAG idea
//!    ("skip extraction for chunks nobody will retrieve"), reduced to a
//!    deterministic, offline gate rather than a learned one. The paper's
//!    sampling skeleton stays out of R2: it needs retrieval telemetry we do
//!    not have yet.
//! 2. **Call counters** (`CostLedger`): per-(stage, model) counts of LLM
//!    calls, extraction attempts, and skipped chunks — observable enough to
//!    decide later whether the learned sampler is worth building.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Which stage a counted call belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CostStage {
    /// Entity/relation extraction (`use_llm_func_with_cache(cache_type="extract")`).
    Extract,
    /// Description summarisation on merge (`cache_type="summary"`).
    Summary,
    /// Query keyword extraction (`cache_type="keywords"`).
    Keywords,
    /// Final answer generation (`cache_type="query"`).
    Query,
}

/// Per-(stage, model) counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostRow {
    pub calls: usize,
    pub cache_hits: usize,
    pub input_tokens: usize,
}

/// Observable cost accounting for one pipeline.
#[derive(Debug, Default)]
pub struct CostLedger {
    rows: Mutex<BTreeMap<(CostStage, String), CostRow>>,
    skipped_chunks: AtomicUsize,
    scored_chunks: AtomicUsize,
}

impl CostLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one model call. `cache_hit` separates fresh calls from cache
    /// reads so a cheap re-run is visible as such.
    pub fn record(&self, stage: CostStage, model: &str, cache_hit: bool, input_tokens: usize) {
        let mut rows = self.rows.lock().expect("cost ledger lock");
        let row = rows.entry((stage, model.to_string())).or_default();
        if cache_hit {
            row.cache_hits += 1;
        } else {
            row.calls += 1;
        }
        row.input_tokens += input_tokens;
    }

    pub fn note_skipped_chunk(&self) {
        self.skipped_chunks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_scored_chunk(&self) {
        self.scored_chunks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn skipped_chunks(&self) -> usize {
        self.skipped_chunks.load(Ordering::Relaxed)
    }

    pub fn scored_chunks(&self) -> usize {
        self.scored_chunks.load(Ordering::Relaxed)
    }

    /// Snapshot of the counters (test/observability surface).
    pub fn rows(&self) -> BTreeMap<(CostStage, String), CostRow> {
        self.rows.lock().expect("cost ledger lock").clone()
    }
}

/// Configuration for the chunk value gate.
#[derive(Debug, Clone, PartialEq)]
pub struct CostOptions {
    /// Skip extraction for chunks shorter than this many tokens
    /// (a one-line chunk rarely carries an entity worth a merge).
    pub min_chunk_tokens: usize,
    /// Skip extraction when a chunk is (near-)duplicate of an already-scored
    /// chunk: the similarity must reach this fraction.
    pub duplicate_similarity: f32,
    /// Hard cap on scored chunks per insert — a runaway document cannot spend
    /// the whole budget on one run.
    pub max_scored_chunks: usize,
}

impl Default for CostOptions {
    fn default() -> Self {
        Self {
            min_chunk_tokens: 24,
            duplicate_similarity: 0.95,
            max_scored_chunks: 5_000,
        }
    }
}

/// Why a chunk was (or was not) kept for extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkVerdict {
    /// Worth an extraction call.
    Keep,
    /// Too short to carry entities.
    TooShort,
    /// Near-duplicate of an earlier chunk.
    Duplicate,
    /// The per-run cap was reached.
    OverBudget,
}

/// Deterministic chunk value scorer (`ShingleSet`-style duplicate detection:
/// cheap, no model, no telemetry).
#[derive(Debug, Default)]
pub struct ChunkValueScorer {
    seen_shingles: Vec<std::collections::HashSet<u64>>,
    scored: usize,
}

impl ChunkValueScorer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Score one chunk. `token_len` is the chunk's token count.
    pub fn score(
        &mut self,
        content: &str,
        token_len: usize,
        options: &CostOptions,
    ) -> ChunkVerdict {
        if token_len < options.min_chunk_tokens {
            return ChunkVerdict::TooShort;
        }
        if self.scored >= options.max_scored_chunks {
            return ChunkVerdict::OverBudget;
        }
        let shingles = shingles(content, 5);
        if shingles.is_empty() {
            return ChunkVerdict::TooShort;
        }
        for seen in &self.seen_shingles {
            if jaccard(seen, &shingles) >= options.duplicate_similarity {
                return ChunkVerdict::Duplicate;
            }
        }
        self.seen_shingles.push(shingles);
        self.scored += 1;
        ChunkVerdict::Keep
    }

    pub fn scored(&self) -> usize {
        self.scored
    }
}

/// Word-level 5-gram hashes (`fnv1a`), the dedup fingerprint.
fn shingles(content: &str, size: usize) -> std::collections::HashSet<u64> {
    let words: Vec<&str> = content.split_whitespace().collect();
    if words.len() < size {
        return words.iter().map(|word| fnv1a(word.as_bytes())).collect();
    }
    words
        .windows(size)
        .map(|window| {
            let mut hasher = Fnv1a::new();
            for word in window {
                hasher.update(word.as_bytes());
                hasher.update(&[0x1f]);
            }
            hasher.finish()
        })
        .collect()
}

fn jaccard(left: &std::collections::HashSet<u64>, right: &std::collections::HashSet<u64>) -> f32 {
    let intersection = left.intersection(right).count();
    let union = left.union(right).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

struct Fnv1a {
    state: u64,
}

impl Fnv1a {
    fn new() -> Self {
        Self {
            state: 0xcbf2_9ce4_8422_2325,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.update(bytes);
    hasher.finish()
}
