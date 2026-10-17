//! The search loop (stage doc 08 S3, T3 SearchStepper): propose one knob at a
//! time, measure it on real cases, and decide whether the candidate may stay.
//!
//! [`Search`] is deliberately small. It owns a [`KnobRegistry`], a tuning set
//! it may look at freely, a holdout set it may only measure, a noise floor the
//! caller declares, and the history of steps it has already accepted. It
//! proposes, it evaluates, it commits. It does not search the space in any
//! clever way — the default strategy is coordinate descent, one knob, one
//! unit step, in a fixed order — because a search whose order depends on
//! anything but the registry cannot be replayed, and a search that cannot be
//! replayed cannot audit a tuning decision.
//!
//! ## The four rules that decide a candidate
//!
//! 1. **Noise floor.** The quality gain on the tuning set must *exceed* the
//!    declared floor. The floor is `1 / (tuning query count)` — the largest
//!    change one query's outcome can make — so a gain clears it only when at
//!    least two queries changed the same way.
//! 2. **Holdout does not regress.** Recall on the holdout set may not fall
//!    below its baseline, and its wrong-evidence rate may not rise.
//! 3. **wrong = 0 is kept.** Both sets must stay at zero wrong evidence and
//!    every case must still meet its frozen conditions.
//! 4. **The ratchet only tightens.** An accepted value becomes the floor for
//!    later steps: [`Search::commit`] moves the registry, records the step,
//!    and a *refuted* direction is never proposed again. A direction refused
//!    by the noise floor alone is not refuted — it was unproductive from that
//!    value — so it stays on offer.
//!
//! ## Why the noise floor is not a repeat-run variance
//!
//! The measurement is deterministic in everything but latency: the same
//! assembly over the same corpus produces the same quality and cost, so the
//! observed spread of repeated runs is zero and *any* gain would clear a
//! variance-derived floor. That is exactly the false positive a floor exists
//! to stop, so the floor is declared in terms of what the case set can
//! actually distinguish — one query's weight — and the caller computes it
//! (`Search::new` takes it; nothing here guesses). A gain equal to the floor
//! is one query's worth of movement and is rejected; two queries' worth is
//! accepted.
//!
//! ## What is not searched
//!
//! The per-call budgets — `recall.top_k`, `recall.rrf_k`,
//! `recall.per_arm_limit` and the staleness pair — are the caller's, not
//! constants the read path owns: `run_case` freezes them per case, so moving
//! them would change what is being measured rather than how the read path
//! reads. The seven knobs [`RecallKnobs`] carries are the searchable surface,
//! and they are the only rows this module ever proposes.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::eval::{run_case_with, Case, CaseResult};
use crate::tune::{KnobRegistry, KnobValue};

/// One candidate: a knob, the value it holds now, and the value a proposer
/// suggests. The rationale is the proposer's own account of why this
/// direction — an attribution stage, or a coordinate direction — so a log
/// line can be read without re-deriving the strategy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub knob_id: String,
    pub from: KnobValue,
    pub to: KnobValue,
    pub rationale: String,
}

/// Why a candidate kept or lost. Every rejection names its rule, because
/// "rejected" without a rule is not actionable in a log and cannot be
/// reviewed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    /// The gain cleared the noise floor and every other rule held.
    Accept { gain: f64 },
    /// The gain did not clear the floor: unproductive *from this value*, not
    /// refuted, so the direction may be proposed again later.
    RejectNoiseFloor { gain: f64 },
    /// A rule other than the floor failed: a frozen condition, wrong
    /// evidence, or the holdout set. Refuted, so the direction stays closed.
    RejectRegression { reason: String },
    /// The candidate value is outside the window the registry states, or of
    /// another kind: rejected before any case runs.
    RejectOutOfBounds { reason: String },
}

/// The ids of the knobs this search may move, in the order it proposes them:
/// registry id order. Split out so the order and the membership are one
/// readable list rather than a filter spread over the code. These are exactly
/// the seven [`RecallKnobs`] fields — the numbers the read path reads as
/// constants — so every id here can be injected through the seam.
const INJECTABLE: [&str; 7] = [
    "recall.graph_hops",
    "recall.jev_candidate_cap",
    "recall.rel_correction",
    "recall.rel_support",
    "recall.rerank_near_zero",
    "recall.seeds_per_arm",
    "recall.summary_chars",
];

/// One search's state: the assembly it is standing on, the two case sets, the
/// floor the caller declared, and the steps it has already accepted.
pub struct Search {
    registry: KnobRegistry,
    tuning: Vec<Case>,
    holdout: Vec<Case>,
    noise_floor: f64,
    accepted: Vec<Step>,
    /// Directions refuted (not merely unprofitable), so a pass never repeats one.
    exhausted: Vec<(String, KnobValue)>,
    /// Both sets' results under the assembly as it stands, measured once and
    /// reused until the assembly moves.
    ///
    /// Every candidate is measured against the same baseline, and the
    /// baseline is a function of the registry — which only `commit` changes.
    /// Re-measuring it per step would double the harness runs of a search for
    /// a number that cannot have moved, so it is cached and dropped on
    /// commit. `None` means "not measured yet".
    baseline: Option<(Vec<CaseResult>, Vec<CaseResult>)>,
}

impl Search {
    /// Start from `registry` over `tuning` (free to look at) and `holdout`
    /// (measured, never mined). `noise_floor` is the caller's declaration of
    /// the smallest quality change worth acting on — see the module head for
    /// why this search does not derive it.
    pub fn new(
        registry: KnobRegistry,
        tuning: Vec<Case>,
        holdout: Vec<Case>,
        noise_floor: f64,
    ) -> Self {
        Self {
            registry,
            tuning,
            holdout,
            noise_floor,
            accepted: Vec::new(),
            exhausted: Vec::new(),
            baseline: None,
        }
    }

    /// The current assembly. After [`Search::commit`] this reflects the
    /// accepted value, which is what a scoreboard records.
    pub fn registry(&self) -> &KnobRegistry {
        &self.registry
    }

    /// The steps this search accepted, in order — the ratchet's record.
    pub fn accepted(&self) -> &[Step] {
        &self.accepted
    }

    /// The next single-knob direction, or `None` when every direction of
    /// every injectable knob has been proposed (the search has converged).
    ///
    /// Pure by construction: registry order, one unit step either side of the
    /// current value, no random, no clock, no IO. The same state always
    /// proposes the same sequence, which is what makes a tuning run auditable.
    ///
    /// A direction already refused stays refused, so a pass never repeats one.
    /// A direction refused by the *noise floor* is not closed — it was merely
    /// unproductive from that value, and a later step from a different value
    /// may legitimately revisit it.
    pub fn propose(&self) -> Option<Step> {
        for id in INJECTABLE {
            let knob = self
                .registry
                .get(id)
                .expect("INJECTABLE names only registered knobs");
            let current = knob.current;
            let bounds = knob.bounds;
            for candidate in self.directions(&current) {
                if self
                    .exhausted
                    .iter()
                    .any(|(tried, value)| tried == id && value == &candidate)
                {
                    continue;
                }
                if !bounds.admits(&candidate) {
                    continue;
                }
                return Some(Step {
                    knob_id: id.to_string(),
                    from: current,
                    to: candidate,
                    rationale: "coordinate descent: one unit step from the current value"
                        .to_string(),
                });
            }
        }
        None
    }

    /// Score `step` and decide it, by the four rules in the module head.
    ///
    /// The rules run cheapest first, and the cheapest is not a measurement: a
    /// candidate outside the registry's window is refused before any case
    /// runs, because it could not be injected anyway and the stores it would
    /// create would be thrown away. An empty set is a rejection rather than a
    /// pass, because a set nobody measured is not evidence of
    /// generalization — and with no tuning set there is nothing to gain.
    pub async fn evaluate(
        &mut self,
        step: &Step,
        workspace_root: &Path,
    ) -> Result<Verdict, String> {
        // Rule 0 (the cheap one): the candidate must be injectable at all.
        let knob = self
            .registry
            .get(&step.knob_id)
            .map_err(|error| error.to_string())?;
        if !knob.bounds.admits(&step.to) {
            self.exhausted.push((step.knob_id.clone(), step.to));
            return Ok(Verdict::RejectOutOfBounds {
                reason: format!(
                    "{:?} is outside this knob's window {:?}",
                    step.to, knob.bounds
                ),
            });
        }
        let mut candidate = self.registry.clone();
        candidate
            .set(&step.knob_id, step.to)
            .map_err(|error| error.to_string())?;

        // An empty set is not a pass. The holdout is what makes an acceptance
        // a generalization rather than an overfit, so with nothing to measure
        // there is nothing to accept.
        if self.tuning.is_empty() {
            return Ok(Verdict::RejectRegression {
                reason: "the tuning set is empty: nothing was measured".to_string(),
            });
        }
        if self.holdout.is_empty() {
            return Ok(Verdict::RejectRegression {
                reason: "the holdout set is empty: a set nobody measured is not generalization"
                    .to_string(),
            });
        }

        // The baseline the step is measured against, in this run: both sets
        // under the assembly as it stands. Cached, because the assembly only
        // moves in `commit` — a search that re-measured it per step would pay
        // for the same number twice per direction.
        if self.baseline.is_none() {
            let tuning = self
                .run_set(&self.registry, "baseline-tuning", workspace_root)
                .await?;
            let holdout = self
                .run_set(&self.registry, "baseline-holdout", workspace_root)
                .await?;
            self.baseline = Some((tuning, holdout));
        }
        let (baseline_tuning, baseline_holdout) = self
            .baseline
            .as_ref()
            .expect("the baseline was just measured");
        let step_tuning = self
            .run_set(&candidate, "step-tuning", workspace_root)
            .await?;
        let step_holdout = self
            .run_set(&candidate, "step-holdout", workspace_root)
            .await?;

        // Rule 3 before the floor: a candidate that breaks a frozen condition
        // or surfaces wrong evidence is a regression whatever it gains.
        for (label, results) in [("tuning", &step_tuning), ("holdout", &step_holdout)] {
            let failed: Vec<&str> = results
                .iter()
                .filter(|result| !result.passed())
                .map(|result| result.case_id.as_str())
                .collect();
            if !failed.is_empty() {
                let reason =
                    format!("{label} set stopped meeting its frozen conditions: {failed:?}");
                self.exhausted.push((step.knob_id.clone(), step.to));
                return Ok(Verdict::RejectRegression { reason });
            }
            let wrong: f64 = results
                .iter()
                .map(|result| result.quality.wrong_evidence_rate)
                .sum();
            if wrong > 0.0 {
                let reason = format!("{label} set surfaced wrong evidence (rate {wrong})");
                self.exhausted.push((step.knob_id.clone(), step.to));
                return Ok(Verdict::RejectRegression { reason });
            }
        }

        // Rule 2: the holdout set may not get worse.
        let holdout_before = recall_of(baseline_holdout);
        let holdout_after = recall_of(&step_holdout);
        if holdout_after < holdout_before {
            let reason =
                format!("the holdout set regressed: recall {holdout_before} -> {holdout_after}");
            self.exhausted.push((step.knob_id.clone(), step.to));
            return Ok(Verdict::RejectRegression { reason });
        }

        // Rule 1: the gain must clear the floor. Equality is a rejection —
        // one query's worth of movement is exactly what the floor is for.
        //
        // The direction is deliberately NOT recorded in `exhausted`: a gain
        // inside the floor is unproductive from *this* value, not refuted, so
        // a step that starts somewhere else may still land on it.
        let tuning_before = recall_of(baseline_tuning);
        let tuning_after = recall_of(&step_tuning);
        let gain = tuning_after - tuning_before;
        if gain <= self.noise_floor {
            return Ok(Verdict::RejectNoiseFloor { gain });
        }

        Ok(Verdict::Accept { gain })
    }

    /// Keep an accepted step: move the registry, record it in the ratchet.
    ///
    /// A step that was not accepted is refused here too — committing a
    /// rejection would make the ratchet a lie.
    pub fn commit(&mut self, step: &Step, verdict: &Verdict) -> Result<(), String> {
        let Verdict::Accept { .. } = verdict else {
            return Err(format!(
                "refusing to commit a step the search did not accept ({verdict:?})"
            ));
        };
        self.registry
            .set(&step.knob_id, step.to)
            .map_err(|error| error.to_string())?;
        self.accepted.push(step.clone());
        // The assembly moved, so the cached baseline is stale: the next step is
        // measured against what this one just produced.
        self.baseline = None;
        Ok(())
    }

    /// One case set under one registry, each case into its own subdirectory
    /// so two assemblies never share a store. `label` ends in `tuning` for
    /// the tuning set and `holdout` for the holdout set, and is also the
    /// directory name, so a run's stores are identifiable after the fact.
    async fn run_set(
        &self,
        registry: &KnobRegistry,
        label: &str,
        workspace_root: &Path,
    ) -> Result<Vec<CaseResult>, String> {
        let knobs = registry.recall_knobs().map_err(|error| error.to_string())?;
        let chosen: &[Case] = if label.ends_with("tuning") {
            &self.tuning
        } else {
            &self.holdout
        };
        let mut results = Vec::with_capacity(chosen.len());
        for case in chosen {
            let workspace = workspace_root.join(format!("{label}-{}", case.case_id));
            let _ = std::fs::remove_dir_all(&workspace);
            results.push(run_case_with(case, &workspace, &knobs).await?);
        }
        Ok(results)
    }

    /// The unit steps either side of `current`, in a fixed order: down first,
    /// then up.
    ///
    /// Integers step by one, so a knob at its floor has no down step and one
    /// at its ceiling has no up step. Floats step by a tenth of their own
    /// magnitude — a scale-free step, because a threshold of 0.05 and a weight
    /// of 1.5 have nothing useful in common as "one unit" — and never below
    /// zero, since every float window in the registry starts there.
    fn directions(&self, current: &KnobValue) -> Vec<KnobValue> {
        match current {
            KnobValue::Usize(value) => {
                let mut out = Vec::new();
                if *value > 0 {
                    out.push(KnobValue::Usize(value - 1));
                }
                out.push(KnobValue::Usize(value + 1));
                out
            }
            KnobValue::U8(value) => {
                let mut out = Vec::new();
                if *value > 0 {
                    out.push(KnobValue::U8(value - 1));
                }
                out.push(KnobValue::U8(value + 1));
                out
            }
            KnobValue::U32(value) => {
                let mut out = Vec::new();
                if *value > 0 {
                    out.push(KnobValue::U32(value - 1));
                }
                out.push(KnobValue::U32(value + 1));
                out
            }
            KnobValue::F64(value) => {
                let step = (value.abs() * 0.1).max(0.1);
                vec![
                    KnobValue::F64((value - step).max(0.0)),
                    KnobValue::F64(value + step),
                ]
            }
            KnobValue::Unset => Vec::new(),
        }
    }
}

/// The pooled recall of a case set — every query counted once, which is the
/// number a tuning decision is made on.
fn recall_of(results: &[CaseResult]) -> f64 {
    let queries: usize = results.iter().map(|result| result.queries.len()).sum();
    if queries == 0 {
        return 0.0;
    }
    let total: f64 = results
        .iter()
        .flat_map(|result| &result.queries)
        .map(|query| query.recall)
        .sum();
    total / queries as f64
}
