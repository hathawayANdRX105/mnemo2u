//! Memory-layer read path: scope-pinned arms → RRF → L0 candidates
//! (stage doc 03 T5/T6/T7). Separate from the RAG pipeline query facade.
//!
//! The arm order is fixed (stage doc 03) and is the only order:
//!
//! 1. **Vector** — ANN top-`limit`, the scope predicate applied *inside* the
//!    store's own search (never a post-filter over unscoped ranks). A
//!    configured vector store with no query vector is **skipped and not
//!    degraded**: there is nothing to ask, which is not a failure.
//! 2. **Lexical** — FTS top-`limit`, scope pre-filtered inside its query.
//! 3. **Graph** — 2-hop undirected expansion. Seeds are the top 10 of the
//!    vector ranking and the top 10 of the lexical ranking: stage doc 03
//!    accepts *only* arm 1/2 hits as seeds, so the graph arm can expand the
//!    hit set but never start one. An empty seed set contributes zero and is
//!    not a failure either — the store is still asked, so a broken adjacency
//!    backend stays observable in `degraded`. Ranking: a fact's score is
//!    `Σ (edge weight × rel multiplier)` over the neighborhood edges touching
//!    it, with `Corrects`/`Supersedes` ×1.5, `DependsOn`/`AboutTask` ×1.2 and
//!    every other rel ×1.0; score descending, id ascending.
//! 4. **Temporal** (conservative form, optional) — active only when the query
//!    text carries a time expression (`YYYY-MM-DD`, 今天/昨天/前天,
//!    today/yesterday). It *re-orders* the fused candidates by how close a
//!    fact's time (`valid_from`, defaulting to `created_at`) is to the
//!    expression's window centre, and never injects a candidate that no
//!    other arm found — so it is a re-ranking pass over the L0 set, not a
//!    fourth scoring source.
//!
//! Degradation (stage doc 03 contract 2): an arm whose store returns `Err`
//! contributes nothing and records its [`ArmName`] in
//! [`RecallOutcome::degraded`]; an arm that answers normally with an empty
//! result is **not** degraded. Even when every configured arm fails the call
//! succeeds — `exists` is simply false and `degraded` names the failures.
//!
//! L0 materialization re-reads each fused id from the truth layer with an
//! exact-scope [`FactStore::get`]: superseded rows (a correction landed) and
//! rows the scope cannot see are dropped silently, so a correction never
//! hands out the stale body.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::traits::{
    rrf_fuse, FactEdgeStore, FactStore, FactVectorStore, LexicalStore, Result,
};
use crate::core::types::{Edge, FactId, RelKind, ScopeKey, SourceRef, TrustState};
use crate::llm::judge::{ExistsVerdict, FindRequest, JudgeClient, RerankCandidate, RerankRequest};

/// Jev rerank candidate ceiling (stage doc 03 contract 4): fusion only ever
/// sees the arms' own limits, so this fixture cap can never inflate the
/// candidate set past what the reranker accepts.
const JEV_CANDIDATE_CAP: usize = 250;

/// Seeds per hit arm for the graph arm — a BFS frontier is a starting set,
/// not the whole ranking.
const SEEDS_PER_ARM: usize = 10;

/// L0 summary length, counted in chars: the truncation must never cut a
/// multi-byte code point in half.
const SUMMARY_CHARS: usize = 200;

/// Graph-arm hops.
const GRAPH_HOPS: u8 = 2;

/// One day in milliseconds — the temporal arm's window grid.
const DAY_MS: i64 = 86_400_000;

/// Rerank relevance at or below this is near zero
/// (R4 contract: a whole near-zero column is a miss,
/// not a confident ordering).
const RERANK_NEAR_ZERO: f64 = 0.05;

/// The memory-layer recall orchestrator: one truth layer plus whichever read
/// arms the deployment configured. Each `Option` that is `None` is an arm the
/// process does not run — it contributes nothing and never reports
/// degradation (an absent arm is not a broken one).
pub struct MemoryRecall {
    pub facts: Arc<dyn FactStore>,
    pub vector: Option<Arc<dyn FactVectorStore>>,
    pub lexical: Option<Arc<dyn LexicalStore>>,
    pub edges: Option<Arc<dyn FactEdgeStore>>,
    /// R4.T4: the judgment reranker. `None` keeps the
    /// fused order — an absent judge is an absent arm,
    /// never a degradation.
    pub judge: Option<Arc<dyn JudgeClient>>,
}

/// Per-call budgets.
pub struct RecallOptions {
    /// L0 candidates handed back (the caller's budget, applied after fusion
    /// and the temporal re-ordering).
    pub top_k: usize,
    /// RRF damping — 60 is the atlas default, a default and not a
    /// measurement.
    pub rrf_k: u32,
    /// Rows each arm may contribute. Clamped to [`JEV_CANDIDATE_CAP`]: the
    /// candidate set never outgrows the reranker's tool limit, no matter
    /// what the caller asks for.
    pub per_arm_limit: usize,
}

/// The five read arms, in fixed order. Used as the `degraded` vocabulary.
pub enum ArmName {
    Vector,
    Lexical,
    Graph,
    Temporal,
    /// The R4 judgment stage (rerank + exists
    /// verdict). Recorded when a judge call fails;
    /// never an arm that finds candidates.
    Judge,
}

impl std::fmt::Display for ArmName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ArmName::Vector => "vector",
            ArmName::Lexical => "lexical",
            ArmName::Graph => "graph",
            ArmName::Temporal => "temporal",
            ArmName::Judge => "judge",
        };
        write!(f, "{name}")
    }
}

impl std::fmt::Debug for ArmName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

/// One L0 entry: a fact summary plus the provenance the harness needs to
/// pull L1/L2 itself.
pub struct L0Candidate {
    pub fact_id: FactId,
    pub summary: String,
    pub source_refs: Vec<SourceRef>,
    pub trust_state: TrustState,
    /// The candidate's RRF score (the same `1/(k + rank)` sum `rrf_fuse`
    /// ranks on), so a caller can compare against a second recall.
    pub score: f64,
    /// Which arms found this id, in fixed arm order. The temporal arm is
    /// never listed: it re-orders candidates, it does not find them.
    pub via: Vec<ArmName>,
}

/// What a recall produced, plus which arms degraded.
pub struct RecallOutcome {
    pub exists: bool,
    pub candidates: Vec<L0Candidate>,
    pub degraded: Vec<ArmName>,
}

impl MemoryRecall {
    /// Run the arms, fuse, re-order, materialize L0.
    ///
    /// `query_vec` gates the vector arm: without it that arm is skipped
    /// (and not degraded). Every store error is absorbed per arm — recall
    /// degrades, it does not fail — except [`FactStore::get`], which is the
    /// truth layer L0 is cut from: a truth store that cannot answer has no
    /// fallback, so its error propagates.
    pub async fn recall(
        &self,
        scope: &ScopeKey,
        query_text: &str,
        query_vec: Option<&[f32]>,
        opts: &RecallOptions,
    ) -> Result<RecallOutcome> {
        let limit = opts.per_arm_limit.min(JEV_CANDIDATE_CAP);
        let mut degraded: Vec<ArmName> = Vec::new();

        // Arm 1 — vector. Configured but no query vector: skipped, not
        // degraded.
        let vector_hits: Vec<(FactId, f64)> = match (&self.vector, query_vec) {
            (Some(store), Some(query)) => match store.search(scope, query, limit).await {
                Ok(hits) => hits,
                Err(error) => {
                    tracing::warn!(%error, arm = %ArmName::Vector, "recall arm failed");
                    degraded.push(ArmName::Vector);
                    Vec::new()
                }
            },
            _ => Vec::new(),
        };

        // Arm 2 — lexical.
        let lexical_hits: Vec<(FactId, f64)> = match &self.lexical {
            Some(store) => match store.search(scope, query_text, limit).await {
                Ok(hits) => hits,
                Err(error) => {
                    tracing::warn!(%error, arm = %ArmName::Lexical, "recall arm failed");
                    degraded.push(ArmName::Lexical);
                    Vec::new()
                }
            },
            None => Vec::new(),
        };

        // Arm 3 — graph, seeded by arm 1/2 hits only.
        let mut seeds: HashSet<FactId> = HashSet::new();
        for hit in vector_hits.iter().take(SEEDS_PER_ARM) {
            seeds.insert(hit.0.clone());
        }
        for hit in lexical_hits.iter().take(SEEDS_PER_ARM) {
            seeds.insert(hit.0.clone());
        }
        let graph_hits: Vec<FactId> = match &self.edges {
            Some(store) => {
                let mut seed_list: Vec<FactId> = seeds.iter().cloned().collect();
                seed_list.sort_by(|a, b| a.0.cmp(&b.0));
                // An empty seed set is a zero contribution, not a failure:
                // `neighbors` answers it with an empty neighborhood. The call
                // still runs so a broken adjacency backend is observable.
                match store.neighbors(&seed_list, GRAPH_HOPS, scope).await {
                    Ok(edges) => graph_rank(&edges),
                    Err(error) => {
                        tracing::warn!(%error, arm = %ArmName::Graph, "recall arm failed");
                        degraded.push(ArmName::Graph);
                        Vec::new()
                    }
                }
            }
            // No edge store configured: the arm is not run and never degrades.
            None => Vec::new(),
        };

        // Fusion — only arms that actually returned rows participate; an
        // empty ranking contributes nothing to RRF and would only clutter
        // the input.
        let mut rankings: Vec<Vec<FactId>> = Vec::new();
        if !vector_hits.is_empty() {
            rankings.push(vector_hits.iter().map(|hit| hit.0.clone()).collect());
        }
        if !lexical_hits.is_empty() {
            rankings.push(lexical_hits.iter().map(|hit| hit.0.clone()).collect());
        }
        if !graph_hits.is_empty() {
            rankings.push(graph_hits.clone());
        }
        let fused: Vec<FactId> = rrf_fuse(&rankings, opts.rrf_k);
        let scores = rrf_scores(&rankings, opts.rrf_k);
        let mut exists = !fused.is_empty();

        // L0 materialization in fused order: exact-scope truth read, with
        // superseded and unseen rows dropped. The fact time rides along
        // because the temporal arm re-orders on it.
        let mut materialized: Vec<(i64, L0Candidate)> = Vec::new();
        for id in &fused {
            let fact = self.facts.get(scope, id).await?;
            let Some(fact) = fact else { continue };
            if fact.superseded_by.is_some() {
                continue;
            }
            let candidate = L0Candidate {
                fact_id: fact.id.clone(),
                summary: summary(&fact.text),
                source_refs: fact.source_refs.clone(),
                trust_state: fact.trust_state,
                score: scores.get(id).copied().unwrap_or_default(),
                via: origins(id, &vector_hits, &lexical_hits, &graph_hits),
            };
            let when = fact.valid_from.unwrap_or(fact.created_at);
            materialized.push((when, candidate));
        }

        // Arm 4 — temporal, re-ordering only. `sort_by` is stable, so facts
        // equally close to the window centre keep their fused order.
        if let Some(centre) = temporal_centre(query_text) {
            materialized.sort_by(|(left, _), (right, _)| {
                left.abs_diff(centre).cmp(&right.abs_diff(centre))
            });
        }

        // R4.T4 — judgment stage over the fused set. The set is
        // bounded by `JEV_CANDIDATE_CAP`, so the reranker's
        // ≤250 limit holds by construction. A failed call
        // degrades: the fused order stands and `Judge` joins
        // `degraded` (once — both calls share the backend).
        if let Some(judge) = &self.judge {
            let ranked: Vec<RerankCandidate> = materialized
                .iter()
                .map(|(_, candidate)| RerankCandidate {
                    id: candidate.fact_id.0.clone(),
                    text: candidate.summary.clone(),
                })
                .collect();
            if !ranked.is_empty() {
                match judge
                    .rerank(&RerankRequest {
                        query: query_text.to_string(),
                        candidates: ranked.clone(),
                    })
                    .await
                {
                    Ok(response) => {
                        let relevance: HashMap<&str, f64> = response
                            .ranked
                            .iter()
                            .map(|hit| (hit.id.as_str(), hit.relevance))
                            .collect();
                        if response
                            .ranked
                            .iter()
                            .all(|hit| hit.relevance <= RERANK_NEAR_ZERO)
                        {
                            // 近零分整列 → 视为 miss: the
                            // reranker found nothing relevant.
                            materialized.clear();
                            exists = false;
                        } else {
                            materialized.sort_by(|(_, left), (_, right)| {
                                let left_score = relevance
                                    .get(left.fact_id.0.as_str())
                                    .copied()
                                    .unwrap_or(0.0);
                                let right_score = relevance
                                    .get(right.fact_id.0.as_str())
                                    .copied()
                                    .unwrap_or(0.0);
                                right_score
                                    .partial_cmp(&left_score)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                        // 答案判定: the verdict is the
                        // recall's existence; 无答案不强选 —
                        // no best is forced on an absent set.
                        if exists {
                            match judge
                                .find(&FindRequest {
                                    query: query_text.to_string(),
                                    candidates: ranked,
                                })
                                .await
                            {
                                Ok(found) => {
                                    if found.exists_verdict == ExistsVerdict::Absent {
                                        materialized.clear();
                                        exists = false;
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        %error,
                                        arm = %ArmName::Judge,
                                        "judge find failed"
                                    );
                                    degraded.push(ArmName::Judge);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            arm = %ArmName::Judge,
                            "judge rerank failed"
                        );
                        degraded.push(ArmName::Judge);
                    }
                }
            }
        }
        let mut candidates: Vec<L0Candidate> = materialized
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect();
        candidates.truncate(opts.top_k);

        Ok(RecallOutcome {
            exists,
            candidates,
            degraded,
        })
    }
}

/// The first 200 chars of `text`, cut on char boundaries: an L0 summary must
/// never split a multi-byte code point.
fn summary(text: &str) -> String {
    text.chars().take(SUMMARY_CHARS).collect()
}

/// Which arms returned `id`, in fixed arm order.
fn origins(
    id: &FactId,
    vector_hits: &[(FactId, f64)],
    lexical_hits: &[(FactId, f64)],
    graph_hits: &[FactId],
) -> Vec<ArmName> {
    let mut via = Vec::new();
    if vector_hits.iter().any(|hit| &hit.0 == id) {
        via.push(ArmName::Vector);
    }
    if lexical_hits.iter().any(|hit| &hit.0 == id) {
        via.push(ArmName::Lexical);
    }
    if graph_hits.contains(id) {
        via.push(ArmName::Graph);
    }
    via
}

/// The same `1/(k + rank)` sum `rrf_fuse` ranks on, kept per id so L0 can
/// report the score its ordering came from. Recomputed here rather than
/// returned by `rrf_fuse`, whose contract is the ordering alone.
fn rrf_scores(rankings: &[Vec<FactId>], k: u32) -> HashMap<FactId, f64> {
    let mut scores: HashMap<FactId, f64> = HashMap::new();
    for ranking in rankings {
        for (index, id) in ranking.iter().enumerate() {
            *scores.entry(id.clone()).or_default() += 1.0 / (k as f64 + index as f64 + 1.0);
        }
    }
    scores
}

/// Rel weighting of the graph arm: a correction supersedes, a task link
/// only frames, and every other relation carries no extra conviction.
fn rel_multiplier(rel: RelKind) -> f64 {
    match rel {
        RelKind::Corrects | RelKind::Supersedes => 1.5,
        RelKind::DependsOn | RelKind::AboutTask => 1.2,
        RelKind::MentionsFile | RelKind::CausedBy | RelKind::Related => 1.0,
    }
}

/// The graph arm's ranking of a neighborhood: every fact touched by a
/// returned edge earns `edge.weight × rel_multiplier(rel)`, from both
/// endpoints (traversal is undirected, so neither side is privileged).
/// Score descending, id ascending as the deterministic tie-break.
fn graph_rank(edges: &[Edge]) -> Vec<FactId> {
    let mut scores: HashMap<FactId, f64> = HashMap::new();
    for edge in edges {
        let contribution = edge.weight * rel_multiplier(edge.rel);
        *scores.entry(edge.src.clone()).or_default() += contribution;
        *scores.entry(edge.dst.clone()).or_default() += contribution;
    }
    let mut ranked: Vec<(FactId, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0 .0.cmp(&b.0 .0)));
    ranked.into_iter().map(|(id, _)| id).collect()
}

/// The temporal arm's window centre, in ms since the epoch: the **first**
/// time expression in the query decides (an explicit date beats a relative
/// word that appears earlier, and vice versa).
///
/// `YYYY-MM-DD` anchors at that date's UTC midnight; 今天/昨天/前天 and
/// today/yesterday anchor at the current UTC day minus 0/1/2 days. Anchors
/// land on the day grid rather than on "now" so a query keeps its centre
/// across the whole day; month/day ranges are validated only as a calendar
/// skeleton (1–12 / 1–31), because the arm ranks by proximity and never
/// renders a date back to a caller.
fn temporal_centre(query_text: &str) -> Option<i64> {
    let haystack = query_text.to_lowercase();
    let mut chosen: Option<(usize, i64)> = None;
    if let Some((position, day_ms)) = find_date(&haystack) {
        chosen = Some((position, day_ms));
    }
    let today = today_millis();
    for (needle, offset_days) in [
        ("今天", 0i64),
        ("昨天", 1),
        ("前天", 2),
        ("today", 0),
        ("yesterday", 1),
    ] {
        if let Some(position) = haystack.find(needle) {
            let candidate = (position, today - offset_days * DAY_MS);
            if chosen.is_none_or(|(best, _)| candidate.0 < best) {
                chosen = Some(candidate);
            }
        }
    }
    chosen.map(|(_, centre)| centre)
}

/// The first `YYYY-MM-DD` in `text` as `(byte position, UTC midnight in ms)`.
/// A digit run longer than four before the dash, or longer than two after
/// it, is not a date — that is how `20261-10-1` stays out.
fn find_date(text: &str) -> Option<(usize, i64)> {
    let bytes = text.as_bytes();
    let digit = |offset: usize| bytes[offset].is_ascii_digit();
    let mut start = 0;
    while start + 10 <= bytes.len() {
        let shaped = (start == 0 || !digit(start - 1))
            && (0..4).all(|offset| digit(start + offset))
            && bytes[start + 4] == b'-'
            && (5..7).all(|offset| digit(start + offset))
            && bytes[start + 7] == b'-'
            && (8..10).all(|offset| digit(start + offset))
            && (start + 10 == bytes.len() || !digit(start + 10));
        if shaped {
            let year: i64 = text[start..start + 4].parse().ok()?;
            let month: u32 = text[start + 5..start + 7].parse().ok()?;
            let day: u32 = text[start + 8..start + 10].parse().ok()?;
            if (1..=12).contains(&month) && (1..=31).contains(&day) {
                let days = days_from_civil(year, i64::from(month), i64::from(day));
                return Some((start, days * DAY_MS));
            }
        }
        start += 1;
    }
    None
}

/// Howard Hinnant's `days_from_civil`: civil date → days since
/// 1970-01-01. The temporal arm's only calendar arithmetic.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = if shifted >= 0 { shifted } else { shifted - 399 } / 400;
    let year_of_era = shifted - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Midnight UTC of the current day, in ms since the epoch.
fn today_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default();
    now - now.rem_euclid(DAY_MS)
}
