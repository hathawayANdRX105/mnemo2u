//! The tuning surface (stage doc 08 S2): what a read-path miss is allowed to
//! turn into, and what a change is measured against.
//!
//! Three pieces, in the order a tuning pass uses them:
//!
//! 1. [`knobs_for`] takes a [`Stage`] from [`crate::diag`] and returns the
//!    knobs that can move it — **only** those. The stage vocabulary is
//!    frozen (stage doc 07), and the mapping is a total `match` over it with
//!    no wildcard arm, so a stage added to `diag` without a knobs decision
//!    stops the build rather than silently tuning nothing. A stage whose
//!    loss is policy or state rather than a number (`InvisibleToScope`,
//!    `NotCaptured`, `NotCurrent`, `Indeterminate`) returns an empty set:
//!    saying "no knob" is the point.
//! 2. [`default_registry`] records the default assembly — id, layer, value,
//!    bounds, rationale per knob. The single source of truth is the code the
//!    knob configures ([`RecallKnobs::default`], `RecallOptions`); this table
//!    is a pinned copy of it, which is why `tests/tune_knobs.rs` compares the
//!    two and a constant moved without its row is red.
//! 3. [`scoreboard_json`] is the comparable artifact: quality and cost
//!    exactly, latency recorded.
//!
//! [`search`] is the fourth: the loop that proposes one knob at a time and
//! decides, against a declared noise floor and a holdout set, whether the
//! candidate may stay.
//!
//! Knobs are injected as a value ([`crate::query::facts::MemoryRecall::
//! recall_with`]), never as process state: there is no environment variable,
//! no config file and no global, so two runs in one process cannot drift
//! into each other and a run's assembly is an argument rather than an
//! ambient fact.
//!
//! ## Why latency is recorded and not compared
//!
//! A wall-clock sample from a CI runner cannot separate "this assembly got
//! slower" from "this runner was busy": the same binary moves by tens of
//! percent between runs, on a shared runner and on a loaded developer
//! machine. Quality and cost are properties of the assembly — a different
//! knob value changes them deterministically — so they are compared exactly.
//! Latency stays in the artifact for a human to read, and the 500 ms p95
//! guard in `tests/eval_gate.rs` stays the tripwire that fails a job.

use serde::{Deserialize, Serialize};

use crate::diag::Stage;
use crate::eval::{CaseResult, Report};
use crate::query::facts::RecallKnobs;

pub mod search;

/// The shape version of the scoreboard artifact. Bump it when a field's
/// meaning changes, so an old baseline cannot be compared against a new one
/// and read as a regression.
const SCHEMA: u32 = 1;

/// The tuning surface a knob belongs to. The partition follows the stage
/// vocabulary: each layer answers one stage family ([`layers_for`]), and no
/// layer exists that no stage reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnobLayer {
    /// [`Stage::AllArmsMissed`] and the arm half of [`Stage::RankedOut`] —
    /// what each arm was asked for, and how the graph arm expands it.
    Arms,
    /// [`Stage::Delivered`] — the L0 view the caller reads. A delivered fact
    /// can still fail an expectation that matches on candidate *text*, and
    /// the summary budget is the read-side number that decides what that
    /// text is. Mapping it here is also what makes the surface complete:
    /// every registered knob is reachable from some stage.
    Delivery,
    /// [`Stage::RankedOut`] — the fused cut and its damping.
    Fusion,
    /// [`Stage::DemotedByJudge`] / [`Stage::JudgeCleared`] — the judgment
    /// stage's own threshold.
    Judge,
    /// [`Stage::StaleFiltered`] — the read-time staleness rule.
    Staleness,
}

/// A knob's value, typed: the type says which knob this is, so a step cannot
/// set a weight to a count without saying so.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum KnobValue {
    /// A `usize` knob — a budget or a character count.
    Usize(usize),
    /// A `u32` knob — the RRF damping.
    U32(u32),
    /// A `u8` knob — the graph arm's depth.
    U8(u8),
    /// An `f64` knob — a threshold or a weight.
    F64(f64),
    /// The default assembly has no value for this knob: the caller supplies
    /// it per call (`RecallOptions`). Recorded as unset rather than guessed,
    /// so the baseline does not pretend to have pinned a budget it never
    /// chose.
    Unset,
}

impl KnobValue {
    /// The value as a number, for a bound check; `None` when unset.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            KnobValue::Usize(value) => Some(*value as f64),
            KnobValue::U32(value) => Some(f64::from(*value)),
            KnobValue::U8(value) => Some(f64::from(*value)),
            KnobValue::F64(value) => Some(*value),
            KnobValue::Unset => None,
        }
    }
}

/// A knob's admissible window, as the range a search step may move it
/// inside. `None` is unbounded on that side: an upper bound is stated only
/// where the code itself has one (a clamp, a comparison domain), never as a
/// guess about what is reasonable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KnobBounds {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl KnobBounds {
    /// Whether `value` sits inside the window. An unset value is admissible
    /// by definition: it is the caller's to supply, not the search's.
    pub fn admits(&self, value: &KnobValue) -> bool {
        let Some(number) = value.as_f64() else {
            return true;
        };
        self.min.is_none_or(|min| number >= min) && self.max.is_none_or(|max| number <= max)
    }
}

/// One registered knob: the name a tuning step uses, the layer it answers,
/// the default assembly's value, the window it may move inside, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Knob {
    /// Dotted and stable, so a step, a baseline diff and a rationale all
    /// name the same thing (`recall.top_k`).
    pub id: &'static str,
    pub layer: KnobLayer,
    pub current: KnobValue,
    pub bounds: KnobBounds,
    /// Why the knob exists and what moving it buys — kept next to the value
    /// so a step is never blind to the contract it is about to change.
    pub rationale: &'static str,
}

/// The registered surface, sorted by id.
///
/// The rows are a pinned copy: the values live in the code the knobs
/// configure, and `tests/tune_knobs.rs` compares every row against
/// `RecallKnobs::default()` — so a constant moved without its row here is a
/// red test, not a registry that quietly disagrees with the build.
static KNOBS: [Knob; 12] = [
    Knob {
        id: "recall.graph_hops",
        layer: KnobLayer::Arms,
        current: KnobValue::U8(2),
        bounds: KnobBounds {
            min: Some(1.0),
            max: None,
        },
        rationale: "Graph-arm traversal depth: deeper walks reach further neighbors at the cost of a larger candidate set.",
    },
    Knob {
        id: "recall.jev_candidate_cap",
        layer: KnobLayer::Arms,
        current: KnobValue::Usize(250),
        bounds: KnobBounds {
            min: Some(1.0),
            max: None,
        },
        rationale: "The rerank candidate ceiling: a caller's per-arm limit is clamped against it, so the candidate set never outgrows what the reranker accepts.",
    },
    Knob {
        id: "recall.per_arm_limit",
        layer: KnobLayer::Arms,
        current: KnobValue::Unset,
        bounds: KnobBounds {
            min: Some(0.0),
            max: Some(250.0),
        },
        rationale: "Rows each arm may contribute; the caller's per-call budget, clamped against recall.jev_candidate_cap.",
    },
    Knob {
        id: "recall.rel_correction",
        layer: KnobLayer::Arms,
        current: KnobValue::F64(1.5),
        bounds: KnobBounds {
            min: Some(0.0),
            max: None,
        },
        rationale: "Graph-arm weight of a Corrects/Supersedes edge: a correction supersedes the row it corrects.",
    },
    Knob {
        id: "recall.rel_support",
        layer: KnobLayer::Arms,
        current: KnobValue::F64(1.2),
        bounds: KnobBounds {
            min: Some(0.0),
            max: None,
        },
        rationale: "Graph-arm weight of a DependsOn/AboutTask edge: a task link only frames a row.",
    },
    Knob {
        id: "recall.rerank_near_zero",
        layer: KnobLayer::Judge,
        current: KnobValue::F64(0.05),
        bounds: KnobBounds {
            min: Some(0.0),
            max: Some(1.0),
        },
        rationale: "Rerank relevance at or below which a whole column is a miss rather than a confident ordering; the domain is a relevance score.",
    },
    Knob {
        id: "recall.rrf_k",
        layer: KnobLayer::Fusion,
        current: KnobValue::U32(60),
        bounds: KnobBounds {
            min: Some(0.0),
            max: None,
        },
        rationale: "RRF damping: 60 is the atlas default the whole tree passes, a default and not a measurement.",
    },
    Knob {
        id: "recall.seeds_per_arm",
        layer: KnobLayer::Arms,
        current: KnobValue::Usize(10),
        bounds: KnobBounds {
            min: Some(1.0),
            max: None,
        },
        rationale: "Graph-arm seeds taken from each hit arm: a BFS frontier is a starting set, not the whole ranking.",
    },
    Knob {
        id: "recall.stale.max_age_secs",
        layer: KnobLayer::Staleness,
        current: KnobValue::Unset,
        bounds: KnobBounds {
            min: Some(0.0),
            max: None,
        },
        rationale: "Age past which a thinly corroborated row is withheld; the caller supplies the rule per call.",
    },
    Knob {
        id: "recall.stale.min_sessions",
        layer: KnobLayer::Staleness,
        current: KnobValue::Unset,
        bounds: KnobBounds {
            min: Some(0.0),
            max: None,
        },
        rationale: "Distinct sessions that revalidate a row older than the age bound; the caller supplies the rule per call.",
    },
    Knob {
        id: "recall.summary_chars",
        layer: KnobLayer::Delivery,
        current: KnobValue::Usize(200),
        bounds: KnobBounds {
            min: Some(1.0),
            max: None,
        },
        rationale: "L0 summary length in chars, cut on char boundaries: a shorter cut can hide a marker a delivered row carries.",
    },
    Knob {
        id: "recall.top_k",
        layer: KnobLayer::Fusion,
        current: KnobValue::Unset,
        bounds: KnobBounds {
            min: Some(0.0),
            max: None,
        },
        rationale: "L0 candidates handed back; the caller's per-case budget, applied after fusion and the temporal re-ordering.",
    },
];

/// Why a knob could not be read or injected. Every failure is an `Err`: an
/// unknown name and a knob without a value are both cases where a guess
/// would be worse than a stop.
///
/// `PartialEq` and not `Eq`: a rejection can name a value, and values carry
/// floats.
#[derive(Debug, Clone, PartialEq)]
pub enum TuneError {
    /// No knob with this id is registered.
    UnknownKnob { id: String },
    /// The knob is registered without a value: the default assembly leaves it
    /// to the caller, so there is nothing to inject.
    UnsetKnob { id: String },
    /// The knob is registered with a value of another kind.
    WrongKind { id: String },
    /// The value sits outside the window the registry states.
    OutOfBounds { id: String, bounds: KnobBounds },
}

impl std::fmt::Display for TuneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TuneError::UnknownKnob { id } => write!(f, "no knob is registered as `{id}`"),
            TuneError::UnsetKnob { id } => write!(f, "knob `{id}` has no value to inject"),
            TuneError::WrongKind { id } => write!(f, "knob `{id}` carries another kind of value"),
            TuneError::OutOfBounds { id, bounds } => {
                write!(f, "value for knob `{id}` is outside its window {bounds:?}")
            }
        }
    }
}

impl std::error::Error for TuneError {}

/// A set of knob values. [`default_registry`] is the default assembly's set;
/// a tuning step (stage doc 08 S3) is another set of the same shape, which
/// is why a scoreboard records the registry it was produced under.
#[derive(Debug, Clone, PartialEq)]
pub struct KnobRegistry {
    knobs: Vec<Knob>,
}

impl KnobRegistry {
    /// Every knob, in id order.
    pub fn knobs(&self) -> &[Knob] {
        &self.knobs
    }

    /// One knob by id. An unknown id is an error, never a panic and never a
    /// silently skipped step: a pass that names a knob nobody registered is
    /// a bug in the pass.
    pub fn get(&self, id: &str) -> Result<&Knob, TuneError> {
        self.knobs
            .iter()
            .find(|knob| knob.id == id)
            .ok_or_else(|| TuneError::UnknownKnob { id: id.to_string() })
    }

    /// This registry as JSON, in id order. Stable by construction: rows are
    /// sorted, values are scalars, and no field is derived at run time — so
    /// the same registry always serializes to the same bytes.
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(&self.knobs)
            .expect("knob records are plain JSON data (no NaN in the table)")
    }

    /// Move one knob to `value`, bounds-checked (stage doc 08 S3: this is the
    /// write half of the surface, and the only one).
    ///
    /// The bounds are the window the registry states, so a value outside them
    /// is an error rather than a silently accepted move — a search step that
    /// could set anything could also set nonsense. An unset row stays unset:
    /// the per-call budgets are the caller's, and this surface refuses to
    /// invent a value for them.
    ///
    /// Rejected: an unknown id, a value outside [`Knob::bounds`],
    /// [`KnobValue::Unset`] written into a row that has no value to clear, a
    /// value of a different kind than the row carries, and any write to a row
    /// that holds no value at all.
    pub fn set(&mut self, id: &str, value: KnobValue) -> Result<(), TuneError> {
        let knob = self.get(id)?;
        let bounds = knob.bounds;
        if matches!(value, KnobValue::Unset) {
            return Err(TuneError::UnsetKnob { id: id.to_string() });
        }
        // Bounds alone let a different kind through: `admits` reads the value
        // as one number whatever its variant, so `F64(2.0)` on a `u8` row is
        // inside the window. It would then fail in `recall_knobs`, long after
        // the write that caused it — so the kind is checked here, where the
        // caller who made the mistake still exists.
        if matches!(knob.current, KnobValue::Unset) {
            // A row with no value has no kind to match, and guessing one would
            // be the silent drift this surface exists to stop.
            return Err(TuneError::UnsetKnob { id: id.to_string() });
        }
        if std::mem::discriminant(&value) != std::mem::discriminant(&knob.current) {
            return Err(TuneError::WrongKind { id: id.to_string() });
        }
        if !bounds.admits(&value) {
            return Err(TuneError::OutOfBounds {
                id: id.to_string(),
                bounds,
            });
        }
        // `get` borrowed `self`, so the row is looked up again to mutate it.
        let knob = self
            .knobs
            .iter_mut()
            .find(|knob| knob.id == id)
            .expect("the id resolved a moment ago");
        knob.current = value;
        Ok(())
    }

    /// The injection half of the surface: this registry's values as the value
    /// [`crate::query::facts::MemoryRecall::recall_with`] takes.
    ///
    /// Only the numbers the read path reads as constants are required. The
    /// per-call budgets (`recall.top_k`, `recall.rrf_k`,
    /// `recall.per_arm_limit`, the staleness pair) are the caller's and stay
    /// in `RecallOptions`; a row that is unset or of another kind is an
    /// error, because injecting a guess is the silent drift this surface
    /// exists to stop.
    pub fn recall_knobs(&self) -> Result<RecallKnobs, TuneError> {
        Ok(RecallKnobs {
            jev_candidate_cap: self.count("recall.jev_candidate_cap")?,
            seeds_per_arm: self.count("recall.seeds_per_arm")?,
            graph_hops: self.depth("recall.graph_hops")?,
            summary_chars: self.count("recall.summary_chars")?,
            rerank_near_zero: self.weight("recall.rerank_near_zero")?,
            rel_correction: self.weight("recall.rel_correction")?,
            rel_support: self.weight("recall.rel_support")?,
        })
    }

    fn count(&self, id: &str) -> Result<usize, TuneError> {
        match self.get(id)?.current {
            KnobValue::Usize(value) => Ok(value),
            KnobValue::Unset => Err(TuneError::UnsetKnob { id: id.to_string() }),
            _ => Err(TuneError::WrongKind { id: id.to_string() }),
        }
    }

    fn depth(&self, id: &str) -> Result<u8, TuneError> {
        match self.get(id)?.current {
            KnobValue::U8(value) => Ok(value),
            KnobValue::Unset => Err(TuneError::UnsetKnob { id: id.to_string() }),
            _ => Err(TuneError::WrongKind { id: id.to_string() }),
        }
    }

    fn weight(&self, id: &str) -> Result<f64, TuneError> {
        match self.get(id)?.current {
            KnobValue::F64(value) => Ok(value),
            KnobValue::Unset => Err(TuneError::UnsetKnob { id: id.to_string() }),
            _ => Err(TuneError::WrongKind { id: id.to_string() }),
        }
    }
}

/// The default assembly's registry: the values stage doc 08 S2 found in the
/// code, with their bounds and rationale.
pub fn default_registry() -> KnobRegistry {
    KnobRegistry {
        knobs: KNOBS.to_vec(),
    }
}

/// The layers that answer a stage, in a fixed order.
///
/// The empty sets are decisions, not omissions: `NotCaptured` is the write
/// path, `NotCurrent` is R5 state, `InvisibleToScope` is a scope policy and
/// `Indeterminate` means the inputs disagree — every one of them is
/// something to decide or fix, not a number to move.
pub fn layers_for(stage: &Stage) -> &'static [KnobLayer] {
    match stage {
        Stage::Delivered => &[KnobLayer::Delivery],
        Stage::NotCaptured => &[],
        Stage::NotCurrent { .. } => &[],
        Stage::InvisibleToScope => &[],
        Stage::StaleFiltered => &[KnobLayer::Staleness],
        Stage::AllArmsMissed => &[KnobLayer::Arms],
        Stage::RankedOut { .. } => &[KnobLayer::Fusion, KnobLayer::Arms],
        Stage::DemotedByJudge { .. } | Stage::JudgeCleared => &[KnobLayer::Judge],
        Stage::Indeterminate => &[],
    }
}

/// The knobs a stage leaves open to tune, in id order (stage doc 08 §五 S3:
/// select the knobs by the attribution, mechanically).
///
/// This is the whole selection rule: a knob is reachable from a stage only
/// through [`layers_for`], so "tune what the diagnosis named" is a property
/// of this function rather than a discipline the caller has to keep.
pub fn knobs_for(stage: &Stage) -> Vec<&'static Knob> {
    let layers = layers_for(stage);
    KNOBS
        .iter()
        .filter(|knob| layers.contains(&knob.layer))
        .collect()
}

/// The scoreboard artifact: the knob set a run was produced under, then the
/// three-axis report.
///
/// Quality and cost are compared exactly — they are the numbers
/// [`Report::from_cases`] already aggregates, so a baseline check is a
/// comparison of serialized values rather than a second definition of the
/// same aggregate. Latency is recorded per case and in the aggregate, and is
/// deliberately outside that comparison (see the module head).
#[derive(Debug, Serialize)]
struct Scoreboard<'a> {
    /// Shape version of this artifact ([`SCHEMA`]).
    schema: u32,
    knobs: &'a [Knob],
    report: Report,
}

/// Summarize a set of case results into the comparable baseline artifact.
///
/// An empty case set is a valid empty scoreboard (zeros on every axis), not
/// an error: the artifact describes what was run, and "nothing" is a
/// describable run. Serialization cannot fail for this shape — every number
/// is finite (each rate divides by a non-empty denominator) — so the
/// artifact is returned directly.
pub fn scoreboard_json(cases: &[CaseResult], knobs: &KnobRegistry) -> String {
    let scoreboard = Scoreboard {
        schema: SCHEMA,
        knobs: knobs.knobs(),
        report: Report::from_cases(cases.to_vec()),
    };
    serde_json::to_string_pretty(&scoreboard)
        .expect("a scoreboard is plain JSON data (every rate divides by a non-empty denominator)")
}
