//! The governance write gateway: closed turns → facts.
//!
//! This is the middle piece that was missing. `judge_write_candidates`
//! (`index/judge.rs`) judges candidates but is a pure function with no
//! caller; `FactStore`/`FactVectorStore`/`LexicalStore`/`FactEdgeStore` can
//! all write but nobody fed them; `SessionIngest` feeds the *RAG* pipeline's
//! doc/chunk tables, not the memory layer. This module connects them.
//!
//! Four responsibilities, in this order (stage doc 04 T4):
//!
//! 1. **Derive** — one closed turn = one candidate, turn text = fact text.
//!    The fact id is `md5(conversation_id \u{1} turn_id)`, so a re-delivered
//!    turn maps to the same row.
//! 2. **Screen** — reuse [`ChunkValueScorer`]: a turn too short or a
//!    near-duplicate of what memory already holds (the batch's candidates
//!    plus the fetched near neighbours) costs **zero** judge calls.
//! 3. **Look up neighbours** — same-scope ANN top-`neighbors`, cosine ≥
//!    `compare_min_cosine`, the rows read from the truth layer so a judge
//!    compares against real text. The candidate itself is excluded.
//! 4. **Judge and commit** — [`judge_write_candidates`] for the survivors,
//!    then per candidate:
//!    - `classify` `Yes` → `Fact.kind` = the picked class, `trust_state` =
//!      [`TrustState::Verified`]; `Review` → default kind, [`TrustState::Candidate`]
//!    - `compare` `SameFact` → corroborate the existing row (append the
//!      new source ref, promote `Candidate → Verified` once two independent
//!      sessions vouch for it) and skip the duplicate write
//!    - `compare` `Contradicts` → supersede chain + a
//!      [`RelKind::Supersedes`] edge (the graph arm weights corrections ×1.5)
//!    - `extract` `Auto` → the value is appended to the fact's *search copy*
//!      (lexical-arm exact recall), never to the stored text
//!    - a value the host explicitly rejected is screened out before any
//!      judge call — the scope-local tombstone ledger (`reject()` writes it,
//!      R5.T3)
//!
//! Commit order follows the repo's established protocol: derived indexes
//! (vector, lexical, edges) first, the truth row (`FactStore`) last, the
//! watermark last of all. A crash in between leaves the watermark behind the
//! data, never ahead, so the next run re-drives the same range and the
//! idempotent writes converge.
//!
//! Judge failure aborts the batch: the caller stops the turn commit and the
//! watermark does not advance — lagging beats silently writing unjudged
//! rows (the mem0 pattern this repo explicitly rejects).
//!
//! Without a judge configured (`judge: None`) the gateway is still usable:
//! the zero-LLM capture path writes every surviving turn as an unverified
//! `episodic` fact with no dedup, no conflict handling and no extraction.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::text::{compute_mdhash_id, Tokenizer};
use crate::core::traits::{
    Embedder, FactEdgeStore, FactStore, FactVectorStore, KvStore, LexicalStore, StoreError,
};
use crate::core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
use crate::index::cost::{ChunkValueScorer, ChunkVerdict, CostOptions};
use crate::index::ingest::{OrganizedThrough, SessionTurn, Turn};
use crate::index::judge::{judge_write_candidates, ComparePassage};
use crate::llm::judge::{ExtractStatus, JudgeClient, JudgeError, Verdict};

/// `Fact.kind` for a turn the judge did not classify. A closed turn is an
/// event by construction; the judge refines this to semantic/procedural when
/// it is confident.
pub const DEFAULT_KIND: &str = "episodic";

/// The `TrustState` a judged fact starts at when classification settled.
const VERIFIED: TrustState = TrustState::Verified;
/// The `TrustState` a fact carries when nothing settled (or no judge ran).
const UNVERIFIED: TrustState = TrustState::Candidate;

/// Distinct sessions required before corroboration promotes a candidate to
/// verified (R5 contract v2; the CSM/cambium lineage — one session vouching
/// for itself is repetition, not corroboration).
const CORROBORATION_THRESHOLD: usize = 2;

/// Compare-neighbour threshold and fan-out defaults (stage doc 04: cosine ≥
/// 0.85, per-scope vector-arm top-10).
const DEFAULT_COMPARE_MIN_COSINE: f64 = 0.85;
const DEFAULT_NEIGHBORS: usize = 10;

/// Where the gateway reads and writes.
pub struct FactWriteGateway {
    facts: Arc<dyn FactStore>,
    vector: Option<Arc<dyn FactVectorStore>>,
    lexical: Option<Arc<dyn LexicalStore>>,
    edges: Option<Arc<dyn FactEdgeStore>>,
    embedder: Arc<dyn Embedder>,
    /// `None` = the zero-LLM capture path (no judge, still writes).
    judge: Option<Arc<dyn JudgeClient>>,
    watermarks: Arc<dyn KvStore>,
    tombstones: Arc<dyn KvStore>,
    tokenizer: Tokenizer,
    options: GatewayOptions,
}

/// Tunables. Every default matches the stage doc.
#[derive(Debug, Clone)]
pub struct GatewayOptions {
    /// The scope every derived fact is written under.
    pub scope: ScopeKey,
    /// Value screening (min tokens, near-duplicate similarity, per-run cap).
    pub cost: CostOptions,
    /// Neighbour cosine floor for the compare step.
    pub compare_min_cosine: f64,
    /// Neighbour fan-out per candidate.
    pub neighbors: usize,
}

impl Default for GatewayOptions {
    fn default() -> Self {
        Self {
            scope: ScopeKey {
                workspace_id: "default".to_string(),
                session_id: None,
                task_id: None,
            },
            cost: CostOptions::default(),
            compare_min_cosine: DEFAULT_COMPARE_MIN_COSINE,
            neighbors: DEFAULT_NEIGHBORS,
        }
    }
}

/// The gateway constructor's argument groups: eight stores/config values
/// stay named at the call site instead of becoming a positional soup.
pub struct GatewayParts {
    pub facts: Arc<dyn FactStore>,
    pub vector: Option<Arc<dyn FactVectorStore>>,
    pub lexical: Option<Arc<dyn LexicalStore>>,
    pub edges: Option<Arc<dyn FactEdgeStore>>,
    pub embedder: Arc<dyn Embedder>,
    /// `None` = the zero-LLM capture path (no judge, still writes).
    pub judge: Option<Arc<dyn JudgeClient>>,
    pub watermarks: Arc<dyn KvStore>,
    /// The scope-local tombstone ledger (R5.T3): value digests the host
    /// explicitly rejected — see [`FactWriteGateway::reject`].
    pub tombstones: Arc<dyn KvStore>,
    pub tokenizer: Tokenizer,
}

impl FactWriteGateway {
    pub fn new(options: GatewayOptions, parts: GatewayParts) -> Self {
        let GatewayParts {
            facts,
            vector,
            lexical,
            edges,
            embedder,
            judge,
            watermarks,
            tombstones,
            tokenizer,
        } = parts;
        Self {
            facts,
            vector,
            lexical,
            edges,
            embedder,
            judge,
            watermarks,
            tombstones,
            tokenizer,
            options,
        }
    }

    /// Drive one batch of closed turns into facts, then advance the
    /// conversation's watermark.
    ///
    /// `conversation_id` keys the watermark row; turns already covered by it
    /// are skipped, so a re-delivered range is a no-op.
    pub async fn commit_turns(
        &self,
        conversation_id: &str,
        turns: &[SessionTurn],
    ) -> Result<GatewayOutcome, GatewayError> {
        let watermark = self.watermark(conversation_id).await?;
        let fresh: Vec<&SessionTurn> = turns
            .iter()
            .filter(|turn| !covered(&watermark, turn))
            .collect();
        if fresh.is_empty() {
            return Ok(GatewayOutcome {
                considered: 0,
                skipped_known: 0,
                skipped_superseded: 0,
                skipped_low_value: 0,
                tombstoned: 0,
                deduplicated: 0,
                corrections: 0,
                written: 0,
                watermark,
            });
        }

        // 1. Derive — one candidate per turn, deterministic id.
        let mut pending: Vec<Pending> = Vec::with_capacity(fresh.len());
        let mut skipped_known = 0usize;
        let mut skipped_superseded = 0usize;
        for turn in &fresh {
            let text = turn.turn.content.trim();
            if text.is_empty() {
                continue;
            }
            let id = FactId(fact_id(conversation_id, &turn.turn));
            match self.facts.get(&self.options.scope, &id).await? {
                // Idempotency: a fact already present for this turn is not
                // re-judged, re-embedded or re-written. A **superseded** row
                // is skipped too — re-writing it would resurrect a
                // correction, which is the R5 "correction survives rebuild"
                // trap. The two cases are counted apart because they mean
                // different things to an operator: one is a replay, the
                // other is a suppressed resurrection.
                Some(existing) if existing.superseded_by.is_none() && existing.text == text => {
                    skipped_known += 1;
                }
                Some(_) => skipped_superseded += 1,
                None => pending.push(Pending {
                    id,
                    conversation_id: conversation_id.to_string(),
                    turn: turn.turn.clone(),
                    text: text.to_string(),
                }),
            }
        }
        if pending.is_empty() {
            return self
                .finish(
                    conversation_id,
                    turns,
                    &fresh,
                    fresh.len(),
                    skipped_known,
                    skipped_superseded,
                    0,
                    0,
                    0,
                    0,
                    0,
                    watermark,
                )
                .await;
        }

        // 2. Embed the batch once: the vectors serve both the neighbour
        // search and the derived vector upsert. Both consumers live inside
        // `if let Some(store) = &self.vector`, so without a vector store the
        // embed has no consumer at all — skip it rather than burn a batch of
        // embed calls the cost axis would have to report.
        let texts: Vec<String> = pending.iter().map(|item| item.text.clone()).collect();
        let embeddings = if self.vector.is_some() {
            self.embedder.embed(&texts).await?
        } else {
            Vec::new()
        };

        // 3. Neighbours, read from the truth layer so the judge compares real
        // text. Self and already-superseded rows never become neighbours.
        let mut neighbours: Vec<Vec<ComparePassage>> = Vec::with_capacity(pending.len());
        let mut neighbour_corpus: Vec<String> = Vec::new();
        for (index, item) in pending.iter().enumerate() {
            let mut passages = Vec::new();
            if let Some(store) = &self.vector {
                let embedding = &embeddings[index];
                let hits = store
                    .search(&self.options.scope, embedding, self.options.neighbors)
                    .await?;
                for (hit_id, similarity) in hits {
                    if hit_id == item.id || similarity < self.options.compare_min_cosine {
                        continue;
                    }
                    let Some(neighbour) = self.facts.get(&self.options.scope, &hit_id).await?
                    else {
                        continue;
                    };
                    if !neighbour.is_current() {
                        continue;
                    }
                    neighbour_corpus.push(neighbour.text.clone());
                    passages.push(ComparePassage {
                        text: neighbour.text,
                        fact_id: neighbour.id,
                    });
                }
            }
            neighbours.push(passages);
        }

        // 4. Screen — the value gate runs against the neighbours first, so a
        // turn duplicating something memory already holds is dropped before
        // any judge call.
        let mut scorer = ChunkValueScorer::new();
        for text in &neighbour_corpus {
            scorer.score(text, self.tokenizer.token_len(text), &self.options.cost);
        }
        let mut survivors: Vec<usize> = Vec::new();
        let mut skipped_low_value = 0usize;
        for (index, item) in pending.iter().enumerate() {
            let token_len = self.tokenizer.token_len(&item.text);
            match scorer.score(&item.text, token_len, &self.options.cost) {
                ChunkVerdict::Keep => survivors.push(index),
                _ => skipped_low_value += 1,
            }
        }

        // 4b. Tombstone screen — a value the host explicitly rejected never
        // re-enters memory, and the check runs before any judge call so a
        // rejected value burns zero model attention (R5.T3, AC3).
        let mut tombstoned = 0usize;
        if !survivors.is_empty() {
            let keys: Vec<String> = survivors
                .iter()
                .map(|&index| tombstone_key(&self.options.scope, &pending[index].text))
                .collect();
            let known = self
                .tombstones
                .get_by_ids(&keys)
                .await
                .map_err(GatewayError::Store)?;
            let mut kept = Vec::with_capacity(survivors.len());
            for (position, &index) in survivors.iter().enumerate() {
                if known[position].is_some() {
                    tombstoned += 1;
                } else {
                    kept.push(index);
                }
            }
            survivors = kept;
        }

        let mut deduplicated = 0usize;
        let mut corrections = 0usize;
        let mut written = 0usize;

        // 5. Judge the survivors, then commit each.
        let judged = match &self.judge {
            Some(judge) => {
                let candidates: Vec<crate::index::judge::FactCandidate> = survivors
                    .iter()
                    .map(|&index| crate::index::judge::FactCandidate {
                        text: pending[index].text.clone(),
                        scope: self.options.scope.clone(),
                        source_refs: vec![pending[index].turn.turn_id.clone()],
                    })
                    .collect();
                let neighbour_lists: Vec<Vec<ComparePassage>> = survivors
                    .iter()
                    .map(|&index| neighbours[index].clone())
                    .collect();
                // A judge failure aborts the batch: nothing has been written
                // yet, so the watermark stays put and the next run retries.
                judge_write_candidates(candidates, neighbour_lists, judge.as_ref())
                    .await
                    .map_err(GatewayError::Judge)?
            }
            None => Vec::new(),
        };

        for (position, survivor_index) in survivors.iter().enumerate() {
            let item = &pending[*survivor_index];
            // `None` when the gateway runs without a judge — the zero-LLM
            // capture path, which still commits the surviving turns.
            let judgement = judgement_or_default(judged.get(position));

            // The search copy keeps the stored text clean and appends the
            // judge's extracted anchors for lexical exact recall.
            let mut search_copy = item.text.clone();
            for result in judgement.extract.values() {
                let auto = result.status == ExtractStatus::Auto;
                if let (Some(value), true) = (result.value.as_ref(), auto) {
                    search_copy.push('\n');
                    search_copy.push_str(&result.field_id);
                    search_copy.push_str(": ");
                    search_copy.push_str(value);
                }
            }

            let contradicts: Vec<&crate::index::judge::CompareResult> = judgement
                .compare
                .iter()
                .filter(|result| {
                    matches!(
                        result.relation,
                        crate::llm::judge::CompareRelation::Contradicts
                    )
                })
                .collect();
            // A duplicate is already in memory: writing it again adds
            // nothing, so it corroborates the row(s) it duplicates (R5
            // contract v2). The new source ref is appended — deduplicated by
            // `(session_id, event_id)`, so a replay of the same evidence adds
            // nothing — and two independent sessions promote the row. A
            // contradiction against a row we are not replacing stays with
            // the supersede path below.
            let same_facts: Vec<&crate::index::judge::CompareResult> = judgement
                .compare
                .iter()
                .filter(|result| {
                    matches!(
                        result.relation,
                        crate::llm::judge::CompareRelation::SameFact
                    )
                })
                .collect();
            if !same_facts.is_empty() {
                let source = self.source_ref_for(&item.conversation_id, &item.turn);
                let mut corroborated = false;
                for result in same_facts {
                    corroborated |= self.corroborate(&result.fact_id, &source).await?;
                }
                // A same-fact target that no longer exists (vanished between
                // the neighbour read and here) falls through to a normal
                // write rather than dropping the evidence silently.
                if corroborated {
                    deduplicated += 1;
                    continue;
                }
            }

            let source_refs = vec![self.source_ref_for(&item.conversation_id, &item.turn)];
            let fact = Fact {
                id: item.id.clone(),
                scope: self.options.scope.clone(),
                kind: judgement
                    .class_id
                    .clone()
                    .unwrap_or_else(|| DEFAULT_KIND.to_string()),
                text: item.text.clone(),
                source_refs,
                trust_state: if judgement.settled() {
                    VERIFIED
                } else {
                    UNVERIFIED
                },
                valid_from: None,
                valid_to: None,
                created_at: item.turn.timestamp,
                superseded_by: None,
                revision: 0,
            };

            // Derived indexes first, the truth row last (the repo's commit
            // protocol): a crash between them leaves invisible derived rows,
            // never a watermark that claims data that is not there.
            if let Some(store) = &self.vector {
                let embedding = &embeddings[*survivor_index];
                store
                    .upsert(&self.options.scope, &item.id, embedding)
                    .await?;
            }
            if let Some(store) = &self.lexical {
                store
                    .index(&self.options.scope, &item.id, &search_copy)
                    .await?;
            }
            if let Some(store) = &self.edges {
                let edges: Vec<Edge> = contradicts
                    .iter()
                    .map(|result| Edge {
                        src: item.id.clone(),
                        dst: result.fact_id.clone(),
                        rel: RelKind::Supersedes,
                        weight: result.confidence,
                        source_event_id: item.turn.turn_id.clone(),
                    })
                    .collect();
                if !edges.is_empty() {
                    store.upsert_edges(&self.options.scope, &edges).await?;
                }
            }

            if contradicts.is_empty() {
                self.facts.put(&fact).await?;
            } else {
                for result in &contradicts {
                    // Guard the re-run: if the chain already landed, the old
                    // row carries `superseded_by`, and superseding again would
                    // bump its revision a second time.
                    let already = self
                        .facts
                        .get(&self.options.scope, &result.fact_id)
                        .await?
                        .is_some_and(|old| old.superseded_by.is_some());
                    if already {
                        self.facts.put(&fact).await?;
                    } else {
                        self.facts.supersede(&result.fact_id, &fact).await?;
                    }
                }
                corrections += 1;
            }
            written += 1;
        }

        self.finish(
            conversation_id,
            turns,
            &fresh,
            fresh.len(),
            skipped_known,
            skipped_superseded,
            skipped_low_value,
            tombstoned,
            deduplicated,
            corrections,
            written,
            watermark,
        )
        .await
    }

    /// Write the watermark for this batch and build the outcome.
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        conversation_id: &str,
        turns: &[SessionTurn],
        _fresh: &[&SessionTurn],
        considered: usize,
        skipped_known: usize,
        skipped_superseded: usize,
        skipped_low_value: usize,
        tombstoned: usize,
        deduplicated: usize,
        corrections: usize,
        written: usize,
        previous: Option<OrganizedThrough>,
    ) -> Result<GatewayOutcome, GatewayError> {
        // Nothing to mark: writing a `{"", 0}` row would overwrite a real
        // ledger with an empty one and make the next run replay everything.
        if turns.is_empty() {
            return Ok(GatewayOutcome {
                considered,
                skipped_known,
                skipped_superseded,
                skipped_low_value,
                tombstoned,
                deduplicated,
                corrections,
                written,
                watermark: previous,
            });
        }
        // The marker is the batch's *latest* turn, not its last array
        // element: `covered` treats `timestamp <= last_timestamp` as done,
        // so a marker taken from an out-of-order tail would regress the
        // ledger and re-drive an already-committed range on the next run.
        // The watermark therefore only ever moves forward.
        let latest = turns.iter().max_by_key(|turn| turn.turn.timestamp);
        let watermark = OrganizedThrough {
            last_turn_id: latest
                .map(|turn| turn.turn.turn_id.clone())
                .unwrap_or_default(),
            last_timestamp: latest.map(|turn| turn.turn.timestamp).unwrap_or_default(),
        };
        let advanced = match &previous {
            Some(previous) if previous.last_timestamp >= watermark.last_timestamp => {
                previous.clone()
            }
            _ => watermark,
        };
        if Some(&advanced) != previous.as_ref() {
            self.watermarks
                .upsert(vec![(
                    conversation_id.to_string(),
                    serde_json::to_value(&advanced).expect("watermark serializes"),
                )])
                .await?;
        }
        Ok(GatewayOutcome {
            considered,
            skipped_known,
            skipped_superseded,
            skipped_low_value,
            tombstoned,
            deduplicated,
            corrections,
            written,
            watermark: Some(advanced),
        })
    }

    /// The conversation's watermark, if any.
    async fn watermark(
        &self,
        conversation_id: &str,
    ) -> Result<Option<OrganizedThrough>, GatewayError> {
        let row = self
            .watermarks
            .get_by_id(conversation_id)
            .await
            .map_err(GatewayError::Store)?;
        Ok(row.and_then(|value| serde_json::from_value(value).ok()))
    }

    /// The provenance pointer for one turn. The turn stream carries no
    /// branch or epoch, so this records the harness's own defaults
    /// (`main`, `0`) — a harness owning the SessionDb supplies richer refs
    /// through its own ledger (C01).
    fn source_ref_for(&self, conversation_id: &str, turn: &Turn) -> SourceRef {
        SourceRef {
            workspace_id: self.options.scope.workspace_id.clone(),
            session_id: self
                .options
                .scope
                .session_id
                .clone()
                .unwrap_or_else(|| conversation_id.to_string()),
            branch_id: "main".to_string(),
            epoch: 0,
            event_id: turn.turn_id.clone(),
        }
    }

    /// Corroborate an existing fact with a new source (R5 contract v2):
    /// the ref is appended unless `(session_id, event_id)` is already
    /// present, so a replay of the same evidence adds nothing; and once
    /// [`CORROBORATION_THRESHOLD`] distinct sessions vouch for the row, a
    /// `Candidate` is promoted to `Verified`. History is preserved — no
    /// revision bump (that counter marks corrections).
    ///
    /// Returns whether the row existed (a vanished target lets the caller
    /// fall through to a normal write instead of dropping the evidence).
    async fn corroborate(&self, id: &FactId, source: &SourceRef) -> Result<bool, GatewayError> {
        let Some(mut fact) = self.facts.get(&self.options.scope, id).await? else {
            return Ok(false);
        };
        let already = fact.source_refs.iter().any(|known| {
            known.session_id == source.session_id && known.event_id == source.event_id
        });
        if already {
            return Ok(true);
        }
        fact.source_refs.push(source.clone());
        let independent = {
            let sessions: BTreeSet<&str> = fact
                .source_refs
                .iter()
                .map(|known| known.session_id.as_str())
                .collect();
            sessions.len()
        };
        if independent >= CORROBORATION_THRESHOLD && fact.trust_state == UNVERIFIED {
            fact.trust_state = VERIFIED;
        }
        self.facts.put(&fact).await?;
        Ok(true)
    }

    /// Explicitly reject a fact's value (R5.T3): the value's canonical
    /// digest enters the scope's tombstone ledger, so no later extraction of
    /// the same value can re-enter memory however the turn is re-delivered;
    /// the row keeps its history with [`TrustState::Rejected`] and a bumped
    /// revision, and leaves recall via [`Fact::is_current`].
    ///
    /// A tombstone never expires (atlas: "a tombstone that expires is not a
    /// tombstone"), and its key is canonicalised — see [`tombstone_key`].
    pub async fn reject(&self, id: &FactId, reason: &str) -> Result<(), GatewayError> {
        let Some(mut fact) = self.facts.get(&self.options.scope, id).await? else {
            return Err(StoreError::NotFound(id.0.clone()).into());
        };
        let key = tombstone_key(&self.options.scope, &fact.text);
        let payload = serde_json::json!({ "reason": reason, "fact_id": fact.id.0 });
        self.tombstones
            .upsert(vec![(key, payload)])
            .await
            .map_err(GatewayError::Store)?;
        fact.trust_state = TrustState::Rejected;
        fact.revision += 1;
        self.facts.put(&fact).await?;
        Ok(())
    }

    /// Invalidate every current fact whose provenance cites the given source
    /// event (R5.T4): a deleted or rewound turn takes its derived facts out
    /// of the current view — `valid_to` closes at `at` (the deletion
    /// instant, supplied by the host) and the revision bumps. Rows that
    /// merely mention the same content are untouched: the blast radius is
    /// exactly the facts that cite this source.
    ///
    /// A re-delivery of the invalidated turn matches the existing row and is
    /// skipped, so invalidation survives replays and re-drives; a different
    /// source re-asserting the value is new evidence and writes a fresh fact
    /// (that is the difference between source invalidation and a value
    /// tombstone).
    ///
    /// Returns the number of rows invalidated.
    pub async fn invalidate_source(
        &self,
        session_id: &str,
        event_id: &str,
        at: i64,
    ) -> Result<usize, GatewayError> {
        let mut invalidated = 0usize;
        for mut fact in self.facts.scan(&self.options.scope).await? {
            if !fact.is_current() {
                continue;
            }
            let cited = fact.source_refs.iter().any(|reference| {
                reference.session_id == session_id && reference.event_id == event_id
            });
            if !cited {
                continue;
            }
            fact.valid_to = Some(at);
            fact.revision += 1;
            self.facts.put(&fact).await?;
            invalidated += 1;
        }
        Ok(invalidated)
    }
}

/// One derived candidate awaiting judgment.
struct Pending {
    id: FactId,
    conversation_id: String,
    turn: Turn,
    text: String,
}

/// Whether a turn is already covered by the watermark. The marker turn
/// itself is covered by definition; everything at or before the
/// watermark's timestamp is too, which is the replay-safety rule a
/// `(turn_id, timestamp)` ledger offers.
fn covered(watermark: &Option<OrganizedThrough>, turn: &SessionTurn) -> bool {
    match watermark {
        None => false,
        Some(mark) => {
            turn.turn.turn_id == mark.last_turn_id || turn.turn.timestamp <= mark.last_timestamp
        }
    }
}

/// Deterministic fact id for a turn: the same turn always maps to the same
/// row, so replay and crash recovery converge.
/// The deterministic fact id the gateway derives for one turn of one
/// conversation.
///
/// `pub(crate)` because the eval harness rewrites every turn id to `t{index}`
/// before it hands the stream over, so a `CaseEdge` has to resolve the
/// position it names against that rewrite to find the facts the store holds.
/// Re-deriving the hash anywhere else would leave a second copy of it to
/// drift.
pub(crate) fn fact_id(conversation_id: &str, turn: &Turn) -> String {
    compute_mdhash_id(
        &format!("{}\u{1}{}", conversation_id, turn.turn_id),
        "fact-",
    )
}

/// The scope-local tombstone key for a value (R5.T3): the text is
/// canonicalised (whitespace collapsed, lowercased) and hashed together
/// with the scope, so the ledger entry can never leak across scopes and a
/// re-extraction of the same value — however it is spaced or cased — lands
/// on the same key. NFKC folding is a registered gap (needs a Unicode
/// dependency); the stage doc carries it.
fn tombstone_key(scope: &ScopeKey, text: &str) -> String {
    let canonical = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    compute_mdhash_id(
        &format!(
            "{}\u{1}{}\u{1}{}\u{1}{}",
            scope.workspace_id,
            scope.session_id.as_deref().unwrap_or_default(),
            scope.task_id.as_deref().unwrap_or_default(),
            canonical
        ),
        "tomb-",
    )
}

/// A judged verdict, or the zero-LLM default when no judge ran.
///
/// When the gateway runs without a judge this is the same shape with
/// `classify = None` (nothing settled) and no class, so the two paths share
/// one commit loop instead of branching.
#[derive(Debug, Clone, Default)]
struct Judgement {
    /// `Some(Verdict::Yes)` is the only settled classification — the only
    /// one that promotes a fact to verified and names a class.
    classify: Option<Verdict>,
    class_id: Option<String>,
    compare: Vec<crate::index::judge::CompareResult>,
    extract: BTreeMap<String, crate::llm::judge::ExtractResult>,
}

impl Judgement {
    fn settled(&self) -> bool {
        self.classify == Some(Verdict::Yes)
    }
}

/// The judged result, or the default when the gateway runs without a judge.
fn judgement_or_default(verdict: Option<&crate::index::judge::JudgedCandidate>) -> Judgement {
    match verdict {
        Some(judged) => Judgement {
            classify: Some(judged.classify),
            class_id: judged.class_id.clone(),
            compare: judged.compare.clone(),
            extract: judged.extract.clone(),
        },
        None => Judgement::default(),
    }
}

/// What one `commit_turns` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayOutcome {
    /// Turns in the batch after the watermark filter.
    pub considered: usize,
    /// Turns whose fact already exists (replay, or a superseded row that
    /// must not be resurrected).
    pub skipped_known: usize,
    /// Turns whose existing row is superseded (or whose text drifted under
    /// the same id): skipped so a correction is never resurrected.
    pub skipped_superseded: usize,
    /// Turns dropped by the value gate — these cost zero judge calls.
    pub skipped_low_value: usize,
    /// Turns whose value matches a tombstone — an explicitly rejected value
    /// never re-enters memory, and this screen also costs zero judge calls.
    pub tombstoned: usize,
    /// Turns the judge called duplicates of existing facts.
    pub deduplicated: usize,
    /// Turns that superseded an earlier fact.
    pub corrections: usize,
    /// Facts committed.
    pub written: usize,
    /// The watermark after this batch.
    pub watermark: Option<OrganizedThrough>,
}

/// Gateway failure: a store error, or a judge error. The two stay distinct
/// on purpose — an `InvalidResponse` means the call was wrong, a transport
/// error means the endpoint is unhealthy (stage doc 04 §降级语义).
#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("judge: {0}")]
    Judge(#[from] JudgeError),
}

/// Re-export so callers name one type for the compare step.
pub use crate::index::judge::ComparePassage as Neighbour;
