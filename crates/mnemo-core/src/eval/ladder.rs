//! The memory ladder (stage doc 07, `todo/design/07-memory-ladder.md`): a set
//! of component tiers, one frozen criteria set, and the marginal-gain-per-MB
//! curve the tiers are read off.
//!
//! The protocol is the four steps of the design doc:
//!
//! 1. **Freeze the criteria.** A [`Criteria`] set is built once and hashed
//!    ([`Criteria::snapshot_id`]). Running a ladder does not change it, and
//!    editing a criterion rotates the hash — so "the same criteria as last
//!    time" is a comparison, not a promise.
//! 2. **Measure every tier.** Each tier answers the frozen criteria and the
//!    run records accuracy, calibration error, option-order flip rate,
//!    abstain rate, latency percentiles and the peak RSS.
//! 3. **Report the curve.** The gain between adjacent tiers is divided by
//!    the megabytes the step costs. The pick per tier is read off the curve,
//!    not asserted in advance.
//! 4. **Let the ladder shrink.** A step with no visible delta over its
//!    predecessor is marked, and the tier it bought is a candidate for
//!    removal from the profile.
//!
//! ## What a tier is
//!
//! A tier names the components it loads ([`Component`]) and the budget it has
//! to fit in ([`Budget`]). The components themselves are *pluggable*:
//! [`ComponentProvider`] is the seam a real model is wired behind later.
//! Nothing here downloads or loads a model — the tiers this module ships
//! with are backed by a deterministic mock (see [`crate::eval::ladder_cases`]).
//!
//! A tier's *added* components are part of its definition, not a comment on
//! it: [`run_ladder`] recomputes the difference against the previous tier
//! and refuses to run when the declaration and the components disagree.
//!
//! ## The MB axis
//!
//! The curve's MB axis is the tier's **declared footprint** — the sum of the
//! megabytes its components declare. That is a property of the tier's
//! definition, so it is identical across runs and comparable across tiers by
//! construction.
//!
//! The **measured** peak RSS is a separate column, and it audits the
//! declaration rather than forming a second axis. A real tier loads its
//! models in its own process, and the framework asks one probe ([`RssProbe`])
//! for the peak of that process. The same probe object serves every tier,
//! which is the protocol's requirement — peak-RSS measurement tooling must be
//! identical across tiers or the MB axis is not comparable. A tier whose peak
//! was not measured says so: [`TierMetrics::peak_rss_bytes`] is `None`, never
//! `0`, so an unmeasured footprint cannot be read as a small one.
//!
//! ## Calibration
//!
//! The calibration error is the definition the judgment training pipeline
//! uses (`laya.common.ece_score`): 15 equal-width bins over `[0, 1]`, each
//! bin contributing its share of the samples times the gap between its mean
//! confidence and its accuracy. A number from here therefore compares with a
//! number from there.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::Latency;

/// The judgment operations the ladder scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// A binary proposition about the state: "is this true or not".
    Noul,
    /// One option out of several.
    Choice,
    /// A level on an ordered scale.
    Score,
}

impl Op {
    /// The op's name in the report and in the criteria.
    pub fn as_str(&self) -> &'static str {
        match self {
            Op::Noul => "noul",
            Op::Choice => "choice",
            Op::Score => "score",
        }
    }
}

/// The language a criterion row is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    En,
    Zh,
}

/// One option a question offers.
///
/// The option id is what the gold label and the component's answer refer to,
/// so it is a stable key rather than a position: the option-order probe
/// permutes the *list* of options, never the ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    /// The stable id (`true`/`false` for a noul question, `0`..`n` for a
    /// score question's levels).
    pub option_id: String,
    /// What the option means, in the row's language.
    pub description: String,
}

/// The frozen answer of one question: a label and a distribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gold {
    /// The option the answer should select.
    pub label: String,
    /// The teacher's probability distribution over the question's options.
    /// The distribution, not the label, is what the soft metrics read.
    pub probabilities: BTreeMap<String, f64>,
}

/// One typed question of a criterion row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionQuestion {
    /// Stable across the whole criteria set.
    pub question_id: String,
    pub op: Op,
    /// The question text, in the row's language.
    pub instructions: String,
    /// The options, in the order the row presents them. The option-order
    /// probe permutes this list.
    pub criteria: Vec<Criterion>,
    pub gold: Gold,
}

/// One judgment state: the evidence plus the questions asked about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionRow {
    /// Stable id of the row.
    pub row_id: String,
    /// The grouping key: every row of one state shares it.
    pub state_hash: String,
    pub lang: Lang,
    /// The evidence payload a component reads.
    pub state: String,
    pub questions: Vec<CriterionQuestion>,
}

/// The frozen criteria set: what every tier is judged against.
///
/// The rows follow the `laya-canon-judgments` row shape — a state, typed
/// questions and a gold distribution per question — so a row can be lifted
/// out of that pipeline without being re-typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criteria {
    pub criteria_id: String,
    pub rows: Vec<CriterionRow>,
    /// The freeze hash. Every row, question and gold is in the payload, so
    /// editing any of them rotates it.
    pub snapshot_id: String,
}

impl Criteria {
    /// Freeze a criteria set and hash it.
    ///
    /// Errors (never a panic): an empty id, an empty row set, a row without
    /// questions, a duplicate row or question id, a question whose gold label
    /// is not one of its options, a gold distribution that does not cover
    /// exactly the question's options or does not sum to one, a gold label
    /// that is not the distribution's most likely option, a row without a
    /// state, a language the set does not cover, or an op the set does not
    /// cover.
    pub fn frozen(criteria_id: impl Into<String>, rows: Vec<CriterionRow>) -> Result<Self, String> {
        let criteria_id = criteria_id.into();
        if criteria_id.trim().is_empty() {
            return Err("the criteria set has no id".to_string());
        }
        if rows.is_empty() {
            return Err(format!("criteria set `{criteria_id}` has no rows"));
        }
        let mut row_ids = Vec::with_capacity(rows.len());
        let mut question_ids = Vec::new();
        let mut languages = Vec::new();
        let mut ops = Vec::new();
        for row in &rows {
            if row.row_id.trim().is_empty() {
                return Err(format!("criteria set `{criteria_id}` has a row with no id"));
            }
            if row_ids.contains(&row.row_id) {
                return Err(format!(
                    "criteria set `{criteria_id}` repeats row id {}",
                    row.row_id
                ));
            }
            row_ids.push(row.row_id.clone());
            if row.state.trim().is_empty() {
                return Err(format!(
                    "criteria set `{criteria_id}`: row {} has no state",
                    row.row_id
                ));
            }
            if row.questions.is_empty() {
                return Err(format!(
                    "criteria set `{criteria_id}`: row {} has no questions",
                    row.row_id
                ));
            }
            languages.push(row.lang);
            for question in &row.questions {
                if question.question_id.trim().is_empty() {
                    return Err(format!(
                        "criteria set `{criteria_id}`: row {} has a question with no id",
                        row.row_id
                    ));
                }
                if question_ids.contains(&question.question_id) {
                    return Err(format!(
                        "criteria set `{criteria_id}` repeats question id {}",
                        question.question_id
                    ));
                }
                question_ids.push(question.question_id.clone());
                ops.push(question.op);
                check_question(&criteria_id, row, question)?;
            }
        }
        for language in [Lang::En, Lang::Zh] {
            if !languages.contains(&language) {
                return Err(format!(
                    "criteria set `{criteria_id}` has no {} rows; the protocol freezes one set per language",
                    match language {
                        Lang::En => "English",
                        Lang::Zh => "Chinese",
                    }
                ));
            }
        }
        for op in [Op::Noul, Op::Choice, Op::Score] {
            if !ops.contains(&op) {
                return Err(format!(
                    "criteria set `{criteria_id}` has no {} question; the protocol freezes one set per operation",
                    op.as_str()
                ));
            }
        }
        Ok(Self {
            criteria_id,
            snapshot_id: criteria_snapshot_id(&rows),
            rows,
        })
    }

    /// Whether this criteria set still carries the snapshot `expected`.
    ///
    /// The comparison a run makes against the previous run's criteria: a
    /// `false` here means the criteria moved under the ladder, which the
    /// protocol forbids.
    pub fn matches_snapshot(&self, expected: &str) -> bool {
        self.snapshot_id == expected
    }
}

/// The freeze hash of a criteria set: every row, question, option and gold.
pub fn criteria_snapshot_id(rows: &[CriterionRow]) -> String {
    let payload = serde_json::json!({ "rows": rows });
    crate::core::text::compute_mdhash_id(&payload.to_string(), "crit-")
}

/// Check one question's internal consistency.
fn check_question(
    criteria_id: &str,
    row: &CriterionRow,
    question: &CriterionQuestion,
) -> Result<(), String> {
    let fail = |reason: String| {
        format!(
            "criteria set `{criteria_id}`: row {} question {}: {reason}",
            row.row_id, question.question_id
        )
    };
    if question.criteria.is_empty() {
        return Err(fail("offers no option".to_string()));
    }
    let mut option_ids = Vec::with_capacity(question.criteria.len());
    for option in &question.criteria {
        if option_ids.contains(&option.option_id) {
            return Err(fail(format!("repeats option id {}", option.option_id)));
        }
        option_ids.push(option.option_id.clone());
    }
    if !option_ids.contains(&question.gold.label) {
        return Err(fail(format!(
            "the gold label {} is not one of its options",
            question.gold.label
        )));
    }
    if question.gold.probabilities.len() != option_ids.len() {
        return Err(fail(format!(
            "the gold distribution covers {} option(s), the question offers {}",
            question.gold.probabilities.len(),
            option_ids.len()
        )));
    }
    let mut total = 0.0;
    for option_id in &option_ids {
        let Some(probability) = question.gold.probabilities.get(option_id) else {
            return Err(fail(format!(
                "the gold distribution misses option {option_id}"
            )));
        };
        if !probability.is_finite() || *probability < 0.0 {
            return Err(fail(format!(
                "option {option_id} has a non-probability {probability}"
            )));
        }
        total += probability;
    }
    if (total - 1.0).abs() > 1e-6 {
        return Err(fail(format!(
            "the gold distribution sums to {total}, not 1"
        )));
    }
    let most_likely = question
        .gold
        .probabilities
        .iter()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .map(|(option_id, _)| option_id.clone())
        .unwrap_or_default();
    if most_likely != question.gold.label {
        return Err(fail(format!(
            "the gold label {} is not the distribution's most likely option ({most_likely})",
            question.gold.label
        )));
    }
    if question.op == Op::Noul {
        let true_probability = question
            .gold
            .probabilities
            .get("true")
            .copied()
            .unwrap_or(0.0);
        if question.gold.label == "true" && true_probability < 0.5 {
            return Err(fail(format!(
                "a noul question labelled `true` carries a probability of {true_probability}"
            )));
        }
    }
    Ok(())
}

/// What a component is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    /// Answers the judgment questions — noul, choice and score alike.
    Judge,
    /// Part of the tier's profile but not exercised by the judgment
    /// criteria. It still counts towards the footprint and the budget.
    Profile,
}

/// One component of a tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    /// The id the component registry resolves.
    pub component_id: String,
    pub kind: ComponentKind,
    /// The component's resident footprint, in MB.
    pub ram_mb: u64,
    /// The component's on-disk footprint, in MB.
    pub disk_mb: u64,
}

/// A tier's budget ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub ram_mb: u64,
    pub disk_mb: u64,
}

/// One tier of the ladder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tier {
    /// Stable id of the tier (`cloud`, `local-mini`, …).
    pub tier_id: String,
    /// The tier's human name.
    pub label: String,
    /// The components this tier loads.
    pub components: Vec<Component>,
    /// The ceiling the tier has to fit in.
    pub budget: Budget,
    /// The component ids this tier adds over the previous one.
    ///
    /// A declaration the runner checks against the components: a tier whose
    /// `adds` does not match what it actually loads is a definition bug, not
    /// a footnote.
    pub adds: Vec<String>,
}

impl Tier {
    /// The tier's declared resident footprint, in MB.
    pub fn footprint_ram_mb(&self) -> u64 {
        self.components
            .iter()
            .map(|component| component.ram_mb)
            .sum()
    }

    /// The tier's declared on-disk footprint, in MB.
    pub fn footprint_disk_mb(&self) -> u64 {
        self.components
            .iter()
            .map(|component| component.disk_mb)
            .sum()
    }

    /// The component that answers the judgment questions, if the tier has
    /// one. A tier without a judge cannot be scored, so it is an error
    /// rather than a tier full of abstentions.
    pub fn judge(&self) -> Option<&Component> {
        self.components
            .iter()
            .find(|component| component.kind == ComponentKind::Judge)
    }
}

/// When a step counts as a visible change.
///
/// A step is visible when *either* axis moved by at least the threshold. The
/// threshold is on the absolute change: a step that made things worse moved
/// as much as a step that made them better, and the protocol's question is
/// whether the tier changed anything at all — a tier that changed nothing is
/// a tier that can be dropped.
///
/// Both thresholds are frozen with the ladder, so the visibility verdict
/// cannot be tuned after the curve is read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VisibilityPolicy {
    /// The smallest accuracy change that counts as visible.
    pub min_accuracy_delta: f64,
    /// The smallest calibration-error change that counts as visible.
    pub min_ece_delta: f64,
}

impl Default for VisibilityPolicy {
    fn default() -> Self {
        Self {
            min_accuracy_delta: 0.01,
            min_ece_delta: 0.01,
        }
    }
}

/// The ladder: an ordered set of tiers plus the rule that reads its curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ladder {
    pub ladder_id: String,
    pub tiers: Vec<Tier>,
    /// How a step decides whether it shows a visible delta.
    pub visibility: VisibilityPolicy,
    /// The freeze hash over the ladder's own definition.
    pub snapshot_id: String,
}

impl Ladder {
    /// Freeze a ladder and hash it.
    ///
    /// Errors (never a panic): an empty id, an empty tier list, a repeated
    /// tier id, a tier with no component, a tier that carries two components
    /// of the same judgment kind (the runner could not tell which one
    /// answers), a tier whose footprint does not fit its budget, or a tier
    /// whose `adds` declaration disagrees with the components it loads.
    pub fn frozen(
        ladder_id: impl Into<String>,
        tiers: Vec<Tier>,
        visibility: VisibilityPolicy,
    ) -> Result<Self, String> {
        let ladder_id = ladder_id.into();
        if ladder_id.trim().is_empty() {
            return Err("the ladder has no id".to_string());
        }
        if tiers.is_empty() {
            return Err(format!("ladder `{ladder_id}` has no tier"));
        }
        let mut tier_ids = Vec::with_capacity(tiers.len());
        let mut previous: Option<&Tier> = None;
        for tier in &tiers {
            if tier.tier_id.trim().is_empty() {
                return Err(format!("ladder `{ladder_id}` has a tier with no id"));
            }
            if tier_ids.contains(&tier.tier_id) {
                return Err(format!(
                    "ladder `{ladder_id}` repeats tier id {}",
                    tier.tier_id
                ));
            }
            tier_ids.push(tier.tier_id.clone());
            if tier.components.is_empty() {
                return Err(format!(
                    "ladder `{ladder_id}`: tier {} loads no component",
                    tier.tier_id
                ));
            }
            // A tier that loads two judges is a tier the runner cannot read:
            // it would have to guess which of them answers.
            let judges = tier
                .components
                .iter()
                .filter(|component| component.kind == ComponentKind::Judge)
                .count();
            if judges > 1 {
                return Err(format!(
                    "ladder `{ladder_id}`: tier {} carries {judges} components of kind {:?}, \
                     so the runner cannot tell which one answers",
                    tier.tier_id,
                    ComponentKind::Judge
                ));
            }
            let ram = tier.footprint_ram_mb();
            let disk = tier.footprint_disk_mb();
            if ram > tier.budget.ram_mb || disk > tier.budget.disk_mb {
                return Err(format!(
                    "ladder `{ladder_id}`: tier {} does not fit its budget: footprint {ram}MB ram / \
                     {disk}MB disk, budget {}MB ram / {}MB disk",
                    tier.tier_id, tier.budget.ram_mb, tier.budget.disk_mb
                ));
            }
            // The `adds` declaration is part of the tier's definition, so it
            // is checked where the definition is frozen. A ladder that is
            // deserialized rather than frozen skips this, which is why the
            // runner checks it a second time.
            if let Some(previous) = previous {
                let (added, _) = component_delta(previous, tier);
                if added != tier.adds {
                    return Err(format!(
                        "ladder `{ladder_id}`: tier {} declares it adds {:?}, but its components make \
                         it add {:?}",
                        tier.tier_id, tier.adds, added
                    ));
                }
            }
            previous = Some(tier);
        }
        Ok(Self {
            ladder_id,
            snapshot_id: ladder_snapshot_id(&tiers, &visibility),
            tiers,
            visibility,
        })
    }
}

/// The freeze hash of a ladder: its tiers, their components and its
/// visibility policy.
pub fn ladder_snapshot_id(tiers: &[Tier], visibility: &VisibilityPolicy) -> String {
    let payload = serde_json::json!({
        "tiers": tiers,
        "visibility": visibility,
    });
    crate::core::text::compute_mdhash_id(&payload.to_string(), "ladder-")
}

/// One judgment, asked of one component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeRequest {
    pub row_id: String,
    pub question_id: String,
    pub op: Op,
    pub lang: Lang,
    /// The evidence payload the component reads.
    pub state: String,
    /// The question text.
    pub instructions: String,
    /// The options, in the order the question presents them.
    ///
    /// The option-order probe asks the same question with this list
    /// reversed, so a component that reads position rather than content
    /// shows up as a flip.
    pub criteria: Vec<Criterion>,
}

/// What a component answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeAnswer {
    /// The option the component selected (choice questions).
    pub choice: Option<String>,
    /// The probability that the proposition holds (noul questions).
    pub noul: Option<f64>,
    /// The level the component settled on (score questions).
    pub score: Option<f64>,
    /// The component's probability mass per option (choice and score
    /// questions). The calibration column falls back to its maximum.
    pub probabilities: BTreeMap<String, f64>,
    /// The component's confidence in its answer, when it states one.
    pub confidence: Option<f64>,
    /// The component declined to answer.
    pub abstain: bool,
}

impl JudgeAnswer {
    /// The option the answer selects, in the question's own vocabulary.
    ///
    /// `None` when the answer carries nothing the op can be read from, which
    /// the runner counts as an abstention rather than a wrong answer.
    pub fn selected(&self, op: Op, criteria: &[Criterion]) -> Option<String> {
        match op {
            Op::Choice => self.choice.clone(),
            Op::Noul => self
                .noul
                .map(|probability| if probability >= 0.5 { "true" } else { "false" }.to_string()),
            Op::Score => self.score.map(|value| level_id(value, criteria.len())),
        }
    }

    /// The confidence the calibration error is computed on.
    ///
    /// The order is the judgment pipeline's: an explicit confidence first,
    /// then the noul margin, then the largest option probability.
    pub fn confidence_for(&self, op: Op) -> f64 {
        if let Some(confidence) = self.confidence {
            return confidence.clamp(0.0, 1.0);
        }
        match op {
            Op::Noul => self
                .noul
                .map(|probability| probability.max(1.0 - probability))
                .unwrap_or(0.0),
            Op::Choice | Op::Score => self.probabilities.values().copied().fold(0.0_f64, f64::max),
        }
    }
}

/// The level id a numeric score lands on.
fn level_id(value: f64, count: usize) -> String {
    let levels = count.max(1) as i64;
    let level = (value.round() as i64).clamp(0, levels - 1);
    level.to_string()
}

/// The pluggable seam: where a tier's components are wired.
///
/// A real component answers here; so does the mock that ships with this
/// module. The trait carries no model identity and no download: the wiring
/// of an actual model is a later change.
#[async_trait]
pub trait ComponentProvider: Send + Sync {
    /// The component id a tier refers to.
    fn component_id(&self) -> &str;
    /// Answer one judgment.
    async fn answer(&self, request: &JudgeRequest) -> Result<JudgeAnswer, String>;
}

/// The components a ladder resolves against.
#[derive(Default)]
pub struct ComponentRegistry {
    providers: BTreeMap<String, Arc<dyn ComponentProvider>>,
}

impl ComponentRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    /// Register a component. A repeated id replaces the earlier one.
    pub fn register(&mut self, provider: Arc<dyn ComponentProvider>) {
        self.providers
            .insert(provider.component_id().to_string(), provider);
    }

    /// The component with this id, if the registry has one.
    pub fn get(&self, component_id: &str) -> Option<&Arc<dyn ComponentProvider>> {
        self.providers.get(component_id)
    }

    /// The registered component ids, in id order.
    pub fn component_ids(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }
}

/// The peak-RSS measurement entry point every tier goes through.
///
/// The protocol requires the measurement to be identical across tiers: a
/// peak-RSS axis assembled from different tools is not an axis. One probe
/// object therefore serves every tier of a run.
pub trait RssProbe: Send + Sync {
    /// The peak resident set size of the tier's process, in bytes.
    ///
    /// `None` means *not measured for this tier*. It never means zero: a
    /// tier whose peak was not read must not look like a tier that needed
    /// no memory.
    fn peak_rss_bytes(&self) -> Option<u64>;
}

/// The probe that measures nothing.
///
/// Every tier it serves reads as *not measured*, which is what a run without
/// a per-tier process has to report.
pub struct UnmeasuredRss;

impl RssProbe for UnmeasuredRss {
    fn peak_rss_bytes(&self) -> Option<u64> {
        None
    }
}

/// The probe that reads this process's peak resident set size.
///
/// It reads `VmHWM` from `/proc/self/status`, which is the kernel's own
/// high-water mark — a single, well-defined entry point rather than a
/// sampling scheme that would differ between tiers. On a platform without
/// `/proc` it reports *not measured*.
///
/// The number is the peak of the **process**, not of one tier inside it: a
/// process that ran several tiers in sequence reports the peak of the whole
/// sequence. A comparable per-tier axis therefore needs one process per
/// tier, with the probe constructed inside that process — which is the shape
/// a real tier loader has, since a tier loads its models into its own
/// process.
pub struct ProcessRss;

impl RssProbe for ProcessRss {
    fn peak_rss_bytes(&self) -> Option<u64> {
        peak_rss_bytes()
    }
}

/// This process's peak resident set size, in bytes.
fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("VmHWM:") else {
            continue;
        };
        let kilobytes: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
        return Some(kilobytes * 1024);
    }
    None
}

/// One operation's metrics inside a tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpMetrics {
    /// Every decision the op was asked for.
    pub decisions: usize,
    /// The decisions the tier answered.
    pub answered: usize,
    /// The share of answered decisions that were correct.
    pub accuracy: f64,
    /// The calibration error over the answered decisions.
    pub ece: f64,
}

/// One tier's metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierMetrics {
    /// Every decision the tier was asked for.
    pub decisions: usize,
    /// The decisions the tier answered. Abstentions are not answers, so they
    /// are out of the accuracy and calibration denominators — and the
    /// abstain rate below is what says how much of the set was skipped.
    pub answered: usize,
    /// The decisions the tier abstained on.
    pub abstained: usize,
    /// The share of answered decisions that were correct.
    pub accuracy: f64,
    /// The calibration error over the answered decisions.
    pub ece: f64,
    /// The share of choice questions whose answer changed when the option
    /// order was reversed.
    pub flip_rate: f64,
    /// The choice questions whose answer changed.
    pub flips: usize,
    /// The choice questions the flip probe could compare.
    pub compared: usize,
    /// The share of all decisions the tier abstained on.
    pub abstain_rate: f64,
    /// The per-operation breakdown.
    pub per_op: BTreeMap<String, OpMetrics>,
    /// The latency of the answered decisions, in microseconds.
    pub latency: Latency,
    /// The peak RSS of the tier's process, when it was measured.
    pub peak_rss_bytes: Option<u64>,
}

/// One tier's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierReport {
    pub tier_id: String,
    pub label: String,
    /// The tier's declared resident footprint, in MB.
    pub footprint_ram_mb: u64,
    /// The tier's declared on-disk footprint, in MB.
    pub footprint_disk_mb: u64,
    pub budget: Budget,
    pub components: Vec<Component>,
    /// The component ids this tier adds over the previous one.
    pub added: Vec<String>,
    /// The component ids this tier drops relative to the previous one.
    pub removed: Vec<String>,
    pub metrics: TierMetrics,
}

/// One step of the marginal-gain curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginalGain {
    pub from_tier: String,
    pub to_tier: String,
    /// The component ids the step adds.
    pub added: Vec<String>,
    /// The component ids the step drops.
    pub removed: Vec<String>,
    /// The accuracy the step buys.
    pub accuracy_delta: f64,
    /// The calibration error the step changes (negative is an improvement).
    pub ece_delta: f64,
    /// The megabytes the step costs on the footprint axis.
    pub footprint_delta_mb: u64,
    /// The megabytes the step costs on the budget axis.
    pub budget_delta_ram_mb: u64,
    /// The accuracy the step buys per additional megabyte. `None` when the
    /// step costs nothing, because a free step has no price to divide by.
    pub gain_per_mb: Option<f64>,
    /// Whether the step shows a visible delta under the ladder's policy.
    pub visible: bool,
}

/// The whole ladder run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderReport {
    pub ladder_id: String,
    pub ladder_snapshot_id: String,
    pub criteria_id: String,
    pub criteria_snapshot_id: String,
    pub visibility: VisibilityPolicy,
    pub tiers: Vec<TierReport>,
    /// The marginal gain between adjacent tiers, one entry per step.
    pub curve: Vec<MarginalGain>,
}

impl LadderReport {
    /// One tier's result, by id.
    pub fn tier(&self, tier_id: &str) -> Option<&TierReport> {
        self.tiers.iter().find(|tier| tier.tier_id == tier_id)
    }

    /// The comparable JSON artifact.
    ///
    /// Plain serialized data — no wall-clock stamp and no run manifest, so
    /// the same ladder over the same criteria produces the same bytes. The
    /// measurements a run produced (latency, peak RSS) are part of the
    /// artifact but are not properties of the assembly, so a comparison
    /// strips them the way the scoreboard baseline strips latency.
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).expect(
            "a ladder report is plain JSON data (every rate divides by a non-empty denominator)",
        )
    }

    /// The readable markdown report.
    ///
    /// The tier table holds the deterministic numbers; the measurement
    /// section at the end holds the run's wall-clock readings, which a
    /// comparison drops.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Memory ladder report\n\n");
        out.push_str(&format!(
            "- ladder: {} (snapshot {})\n",
            self.ladder_id, self.ladder_snapshot_id
        ));
        out.push_str(&format!(
            "- criteria: {} (snapshot {})\n",
            self.criteria_id, self.criteria_snapshot_id
        ));
        out.push_str(&format!("- tiers: {}\n", self.tiers.len()));
        out.push_str(&format!(
            "- visibility policy: accuracy delta >= {:.4}, ece delta >= {:.4}\n",
            self.visibility.min_accuracy_delta, self.visibility.min_ece_delta
        ));
        out.push('\n');
        out.push_str("## Tiers\n\n");
        out.push_str("| tier | label | footprint ram/disk (MB) | budget ram/disk (MB) | added | removed | accuracy | ece | flip rate | abstain rate | decisions | answered |\n");
        out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        for tier in &self.tiers {
            out.push_str(&format!(
                "| {} | {} | {} / {} | {} / {} | {} | {} | {:.4} | {:.4} | {:.4} | {:.4} | {} | {} |\n",
                tier.tier_id,
                tier.label,
                tier.footprint_ram_mb,
                tier.footprint_disk_mb,
                tier.budget.ram_mb,
                tier.budget.disk_mb,
                join(&tier.added),
                join(&tier.removed),
                tier.metrics.accuracy,
                tier.metrics.ece,
                tier.metrics.flip_rate,
                tier.metrics.abstain_rate,
                tier.metrics.decisions,
                tier.metrics.answered,
            ));
        }
        out.push('\n');
        out.push_str("## Marginal gain per MB\n\n");
        out.push_str("| step | added | removed | delta accuracy | delta ece | delta footprint (MB) | delta budget (MB) | gain per MB | visible |\n");
        out.push_str("|---|---|---|---|---|---|---|---|---|\n");
        for step in &self.curve {
            out.push_str(&format!(
                "| {} -> {} | {} | {} | {:.4} | {:.4} | {} | {} | {} | {} |\n",
                step.from_tier,
                step.to_tier,
                join(&step.added),
                join(&step.removed),
                step.accuracy_delta,
                step.ece_delta,
                step.footprint_delta_mb,
                step.budget_delta_ram_mb,
                match step.gain_per_mb {
                    Some(gain) => format!("{gain:.6}"),
                    None => "n/a".to_string(),
                },
                if step.visible { "yes" } else { "no" },
            ));
        }
        out.push('\n');
        out.push_str("## Per-operation accuracy\n\n");
        out.push_str("| tier | noul | choice | score |\n");
        out.push_str("|---|---|---|---|\n");
        for tier in &self.tiers {
            let cell = |op: Op| {
                tier.metrics
                    .per_op
                    .get(op.as_str())
                    .map(|metrics| format!("{:.4}", metrics.accuracy))
                    .unwrap_or_else(|| "n/a".to_string())
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                tier.tier_id,
                cell(Op::Noul),
                cell(Op::Choice),
                cell(Op::Score),
            ));
        }
        out.push('\n');
        out.push_str("## Measurements\n\n");
        out.push_str("Recorded by this run and not compared: a wall-clock reading is a property of the machine, not of the assembly.\n\n");
        out.push_str("| tier | latency p50 (us) | latency p95 (us) | peak rss (bytes) |\n");
        out.push_str("|---|---|---|---|\n");
        for tier in &self.tiers {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                tier.tier_id,
                tier.metrics.latency.p50_us,
                tier.metrics.latency.p95_us,
                match tier.metrics.peak_rss_bytes {
                    Some(bytes) => bytes.to_string(),
                    None => "not measured".to_string(),
                },
            ));
        }
        out
    }
}

/// A list of ids as one cell.
fn join(ids: &[String]) -> String {
    if ids.is_empty() {
        "-".to_string()
    } else {
        ids.join(", ")
    }
}

/// Run the ladder: every tier against the frozen criteria.
///
/// The criteria are read, never written: the run cannot change what it is
/// judged against, and the report echoes the criteria's snapshot so the next
/// run can compare against it.
///
/// Errors (never a panic): a tier whose `adds` does not match the components
/// it actually loads, a tier that references a component the registry does
/// not have, a component that fails to answer, or a tier that answered
/// nothing at all — a tier with no answered decision has no accuracy and no
/// calibration error to report.
pub async fn run_ladder(
    ladder: &Ladder,
    criteria: &Criteria,
    registry: &ComponentRegistry,
    rss: &dyn RssProbe,
) -> Result<LadderReport, String> {
    let mut tiers = Vec::with_capacity(ladder.tiers.len());
    let mut previous: Option<&Tier> = None;
    for tier in &ladder.tiers {
        let (added, removed) = match previous {
            None => (Vec::new(), Vec::new()),
            Some(previous) => component_delta(previous, tier),
        };
        if added != tier.adds {
            return Err(format!(
                "ladder `{}`: tier {} declares it adds {:?}, but its components make it add {:?}",
                ladder.ladder_id, tier.tier_id, tier.adds, added
            ));
        }
        let mut metrics = run_tier(tier, criteria, registry).await?;
        metrics.peak_rss_bytes = rss.peak_rss_bytes();
        tiers.push(TierReport {
            tier_id: tier.tier_id.clone(),
            label: tier.label.clone(),
            footprint_ram_mb: tier.footprint_ram_mb(),
            footprint_disk_mb: tier.footprint_disk_mb(),
            budget: tier.budget,
            components: tier.components.clone(),
            added,
            removed,
            metrics,
        });
        previous = Some(tier);
    }

    let curve = build_curve(&tiers, ladder.visibility);
    Ok(LadderReport {
        ladder_id: ladder.ladder_id.clone(),
        ladder_snapshot_id: ladder.snapshot_id.clone(),
        criteria_id: criteria.criteria_id.clone(),
        criteria_snapshot_id: criteria.snapshot_id.clone(),
        visibility: ladder.visibility,
        tiers,
        curve,
    })
}

/// The component ids a tier adds and drops relative to the previous one.
fn component_delta(previous: &Tier, tier: &Tier) -> (Vec<String>, Vec<String>) {
    let previous_ids: Vec<&str> = previous
        .components
        .iter()
        .map(|component| component.component_id.as_str())
        .collect();
    let tier_ids: Vec<&str> = tier
        .components
        .iter()
        .map(|component| component.component_id.as_str())
        .collect();
    let added = tier_ids
        .iter()
        .filter(|id| !previous_ids.contains(id))
        .map(|id| id.to_string())
        .collect();
    let removed = previous_ids
        .iter()
        .filter(|id| !tier_ids.contains(id))
        .map(|id| id.to_string())
        .collect();
    (added, removed)
}

/// The marginal gain between adjacent tiers.
fn build_curve(tiers: &[TierReport], visibility: VisibilityPolicy) -> Vec<MarginalGain> {
    let mut curve = Vec::new();
    for pair in tiers.windows(2) {
        let (previous, tier) = (&pair[0], &pair[1]);
        let accuracy_delta = tier.metrics.accuracy - previous.metrics.accuracy;
        let ece_delta = tier.metrics.ece - previous.metrics.ece;
        let footprint_delta_mb = tier
            .footprint_ram_mb
            .saturating_sub(previous.footprint_ram_mb);
        let budget_delta_ram_mb = tier.budget.ram_mb.saturating_sub(previous.budget.ram_mb);
        let visible = accuracy_delta.abs() >= visibility.min_accuracy_delta
            || ece_delta.abs() >= visibility.min_ece_delta;
        curve.push(MarginalGain {
            from_tier: previous.tier_id.clone(),
            to_tier: tier.tier_id.clone(),
            added: tier.added.clone(),
            removed: tier.removed.clone(),
            accuracy_delta,
            ece_delta,
            footprint_delta_mb,
            budget_delta_ram_mb,
            gain_per_mb: if footprint_delta_mb == 0 {
                None
            } else {
                Some(accuracy_delta / footprint_delta_mb as f64)
            },
            visible,
        });
    }
    curve
}

/// The decisions of one operation, before the metrics are derived.
#[derive(Default)]
struct OpAccumulator {
    decisions: usize,
    answered: usize,
    correct: usize,
    calibration: Vec<(f64, bool)>,
}

/// Run one tier over the frozen criteria.
async fn run_tier(
    tier: &Tier,
    criteria: &Criteria,
    registry: &ComponentRegistry,
) -> Result<TierMetrics, String> {
    let mut samples: Vec<u128> = Vec::new();
    let mut answered = 0usize;
    let mut abstained = 0usize;
    let mut correct = 0usize;
    let mut calibration: Vec<(f64, bool)> = Vec::new();
    let mut flips = 0usize;
    let mut compared = 0usize;
    let mut per_op: BTreeMap<String, OpAccumulator> = BTreeMap::new();

    let judge = tier.judge().ok_or_else(|| {
        format!(
            "tier {} loads no judge component, so it cannot answer the criteria",
            tier.tier_id
        )
    })?;
    let provider = registry.get(&judge.component_id).ok_or_else(|| {
        format!(
            "tier {}: no provider registered for component {}",
            tier.tier_id, judge.component_id
        )
    })?;

    for row in &criteria.rows {
        for question in &row.questions {
            let accumulator = per_op.entry(question.op.as_str().to_string()).or_default();
            accumulator.decisions += 1;

            let request = JudgeRequest {
                row_id: row.row_id.clone(),
                question_id: question.question_id.clone(),
                op: question.op,
                lang: row.lang,
                state: row.state.clone(),
                instructions: question.instructions.clone(),
                criteria: question.criteria.clone(),
            };
            let started = Instant::now();
            let answer = provider.answer(&request).await.map_err(|error| {
                format!(
                    "tier {}: component {} failed on question {}: {error}",
                    tier.tier_id, judge.component_id, question.question_id
                )
            })?;
            samples.push(started.elapsed().as_micros());

            if answer.abstain {
                abstained += 1;
                continue;
            }
            answered += 1;
            accumulator.answered += 1;
            let selected = answer.selected(question.op, &question.criteria);
            let is_correct = selected.as_deref() == Some(question.gold.label.as_str());
            if is_correct {
                correct += 1;
                accumulator.correct += 1;
            }
            let confidence = answer.confidence_for(question.op);
            calibration.push((confidence, is_correct));
            accumulator.calibration.push((confidence, is_correct));

            if question.op == Op::Choice && question.criteria.len() > 1 {
                let reversed = JudgeRequest {
                    criteria: question.criteria.iter().rev().cloned().collect(),
                    ..request.clone()
                };
                let flipped = provider.answer(&reversed).await.map_err(|error| {
                    format!(
                        "tier {}: component {} failed on the option-order probe of question {}: {error}",
                        tier.tier_id, judge.component_id, question.question_id
                    )
                })?;
                if !flipped.abstain {
                    let before = selected.as_deref();
                    let after = flipped.selected(Op::Choice, &question.criteria);
                    if let Some(after) = after {
                        compared += 1;
                        if Some(after.as_str()) != before {
                            flips += 1;
                        }
                    }
                }
            }
        }
    }

    if answered == 0 {
        return Err(format!(
            "tier {} answered none of its {} decision(s), so it has no accuracy and no \
             calibration error to report",
            tier.tier_id,
            answered + abstained
        ));
    }

    let per_op: BTreeMap<String, OpMetrics> = per_op
        .into_iter()
        .map(|(op, accumulator)| {
            let accuracy = if accumulator.answered == 0 {
                0.0
            } else {
                accumulator.correct as f64 / accumulator.answered as f64
            };
            (
                op,
                OpMetrics {
                    decisions: accumulator.decisions,
                    answered: accumulator.answered,
                    accuracy,
                    ece: expected_calibration_error(&accumulator.calibration, ECE_BINS),
                },
            )
        })
        .collect();

    let decisions = answered + abstained;
    let accuracy = correct as f64 / answered as f64;
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    Ok(TierMetrics {
        decisions,
        answered,
        abstained,
        accuracy,
        ece: expected_calibration_error(&calibration, ECE_BINS),
        flip_rate: if compared == 0 {
            0.0
        } else {
            flips as f64 / compared as f64
        },
        flips,
        compared,
        abstain_rate: if decisions == 0 {
            0.0
        } else {
            abstained as f64 / decisions as f64
        },
        per_op,
        latency: Latency {
            samples: sorted.len(),
            p50_us: sorted.get(sorted.len() / 2).copied().unwrap_or(0),
            p95_us: sorted.get((sorted.len() * 95) / 100).copied().unwrap_or(0),
        },
        peak_rss_bytes: None,
    })
}

/// The number of equal-width bins the calibration error is computed over.
///
/// The judgment pipeline's number (`laya.common.ece_score` defaults to 15),
/// so a number from here compares with a number from there.
pub const ECE_BINS: usize = 15;

/// The expected calibration error over `(confidence, correct)` pairs.
///
/// The definition is the judgment pipeline's: 15 equal-width bins over
/// `[0, 1]`; each bin contributes its share of the samples times the gap
/// between its mean confidence and its accuracy. The first bin includes its
/// lower edge, the others do not, so a confidence of exactly `0` lands in
/// the first bin rather than in none.
pub fn expected_calibration_error(samples: &[(f64, bool)], bins: usize) -> f64 {
    if samples.is_empty() || bins == 0 {
        return 0.0;
    }
    let mut error = 0.0;
    for index in 0..bins {
        let low = index as f64 / bins as f64;
        let high = (index + 1) as f64 / bins as f64;
        let selected: Vec<(f64, bool)> = samples
            .iter()
            .copied()
            .filter(|(confidence, _)| {
                let above = if index == 0 {
                    *confidence >= low
                } else {
                    *confidence > low
                };
                above && *confidence <= high
            })
            .collect();
        if selected.is_empty() {
            continue;
        }
        let count = selected.len();
        let mean_confidence: f64 = selected
            .iter()
            .map(|(confidence, _)| *confidence)
            .sum::<f64>()
            / count as f64;
        let accuracy = selected
            .iter()
            .filter(|(_, is_correct)| *is_correct)
            .count() as f64
            / count as f64;
        error += (count as f64 / samples.len() as f64) * (mean_confidence - accuracy).abs();
    }
    error
}
