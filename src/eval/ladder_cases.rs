//! The shipped ladder: frozen criteria in both languages, and the mock
//! components that stand in for models this repository does not have.
//!
//! ## What is real and what is mocked
//!
//! The **criteria** are real: they are the frozen judgment rows, in the
//! `laya-canon-judgments` shape — a state, typed questions, a gold
//! distribution — written in English and Chinese and covering all three
//! operations. They are the thing the protocol freezes.
//!
//! The **components** are mocked. The design doc's tier table names
//! KaLM-Jev, OpenDecider, laya, blink, potion and bert-tiny-mnli, and not
//! one of them exists in this repository — nor may a test download one. So
//! each component here is a deterministic stand-in that answers the frozen
//! criteria with a known, tier-dependent quality. What the ladder measures
//! is therefore the *protocol*, not a model: the numbers say how the curve
//! is read, and they are replaced the moment a real component is wired
//! behind [`ComponentProvider`].
//!
//! ## How the mock decides
//!
//! A mock holds the gold answers, because it stands in for a model that has
//! been trained on them. On each question it draws two deterministic values
//! from the question's id — one says how *hard* the question is, the other
//! how *reluctant* the component is — and then:
//!
//! - when the difficulty is below the component's skill, it knows the
//!   answer and answers the gold option, in any option order;
//! - otherwise it guesses, and its guess is drawn from the option list **as
//!   presented**. That is what makes the option-order probe meaningful:
//!   reversing the list changes the guess, and a component that answers by
//!   position shows up as a flip.
//!
//! The draws are keyed by the **question**, not by the component, so a
//! stronger judge knows a *superset* of what a weaker one knows and a
//! less reluctant one answers a superset of what the reluctant one answers.
//! The ladder's curve is therefore a property of the design rather than of a
//! hash lottery: a stronger tier is never worse on any single question. The
//! component's own behaviour is the threshold its draws are compared
//! against, so two components that behave the same answer the same — which
//! is what lets the ladder show a tier that is the previous tier again under
//! a bigger footprint.
//!
//! ## The shipped tiers
//!
//! | tier | judge | footprint | what it is for |
//! |---|---|---|---|
//! | `cloud` | `judge-cloud` | 0 MB | a remote endpoint: no local model at all |
//! | `local-nano` | `judge-nano` | 40 MB | a small local judge |
//! | `local-mini` | `judge-mini` | 100 MB | a larger local judge |
//! | `local-mini-fp16` | `judge-mini-fp16` | 180 MB | the same judge in a wider build |
//!
//! The last tier is deliberately the shape the protocol's fourth step exists
//! for: it costs 80 MB more than `local-mini` and behaves exactly like it,
//! so its step shows no visible delta and the tier is a candidate for
//! removal from the profile.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::ladder::{
    Budget, Component, ComponentKind, ComponentProvider, ComponentRegistry, Criteria, Criterion,
    CriterionQuestion, CriterionRow, Gold, JudgeAnswer, JudgeRequest, Ladder, Lang, Op, Tier,
    VisibilityPolicy,
};

/// The marker a state carries so the mock knows the answer is in there.
const ANSWER_MARKER: &str = "ANSWER:";

/// The frozen criteria set: English and Chinese rows, all three operations.
///
/// The set is deliberately modest — a ladder report is read by a human — but
/// it is wide enough to separate the tiers: three questions cannot tell a
/// 55% judge from a 78% one, and a ladder that cannot separate its tiers
/// cannot show a curve. What matters here is that the set is *frozen*: same
/// rows, same hash, every run.
pub fn frozen_criteria() -> Criteria {
    Criteria::frozen("canon-judgments-en-zh", rows())
        .expect("the shipped criteria set is consistent")
}

/// One option of a choice question: the id an answer refers to, and what the
/// option means in the row's language.
struct ChoiceOption {
    id: &'static str,
    description: &'static str,
}

/// One judgment topic: the evidence a component reads, and the three
/// questions asked about it.
struct Topic {
    /// The row's id.
    row_id: &'static str,
    /// The language the row is written in.
    lang: Lang,
    /// The evidence, carrying the answer the component has to find.
    state: &'static str,
    /// The noul question, and whether the state settles it true.
    noul: (&'static str, bool),
    /// The choice question: its text, the options it offers in the order
    /// they are presented, and the index of the right one.
    choice: (&'static str, [ChoiceOption; 3], usize),
    /// The score question, and the level it settles on.
    score: (&'static str, u8),
}

/// The topics the shipped rows are built from.
///
/// Every topic is a fact the state settles, asked about three ways: a
/// proposition that is or is not true, a choice among three options, and a
/// level on the settledness scale. The option *order* differs from topic to
/// topic, so a component that answered by position rather than by content
/// would not be right on every row.
const TOPICS: [Topic; 8] = [
    Topic {
        row_id: "en-0001",
        lang: Lang::En,
        state:
            "The deploy key rotates every ninety days and the rotation policy is fully settled. \
                ANSWER: ninety days",
        noul: ("Is the rotation window ninety days?", true),
        choice: (
            "Which rotation window was settled?",
            [
                ChoiceOption {
                    id: "ninety-days",
                    description: "the window is ninety days",
                },
                ChoiceOption {
                    id: "thirty-days",
                    description: "the window is thirty days",
                },
                ChoiceOption {
                    id: "one-year",
                    description: "the window is one year",
                },
            ],
            0,
        ),
        score: ("How settled is the rotation policy?", 2),
    },
    Topic {
        row_id: "en-0002",
        lang: Lang::En,
        state:
            "Nightly backups are kept for thirty days and the retention policy is fully settled. \
                ANSWER: thirty days",
        noul: ("Are backups kept for thirty days?", true),
        choice: (
            "How long are backups kept?",
            [
                ChoiceOption {
                    id: "seven-days",
                    description: "the window is seven days",
                },
                ChoiceOption {
                    id: "thirty-days",
                    description: "the window is thirty days",
                },
                ChoiceOption {
                    id: "ninety-days",
                    description: "the window is ninety days",
                },
            ],
            1,
        ),
        score: ("How settled is the retention policy?", 2),
    },
    Topic {
        row_id: "en-0003",
        lang: Lang::En,
        state: "The primary region is eu-west-1 and the region choice is only partly settled. \
                ANSWER: eu-west-1",
        noul: ("Is the primary region eu-west-1?", true),
        choice: (
            "Which region serves the primary traffic?",
            [
                ChoiceOption {
                    id: "us-east-1",
                    description: "the region is us-east-1",
                },
                ChoiceOption {
                    id: "eu-west-1",
                    description: "the region is eu-west-1",
                },
                ChoiceOption {
                    id: "ap-south-1",
                    description: "the region is ap-south-1",
                },
            ],
            1,
        ),
        score: ("How settled is the region choice?", 1),
    },
    Topic {
        row_id: "en-0004",
        lang: Lang::En,
        state: "Payment changes need two approvals and the approval rule is fully settled. \
                ANSWER: two approvals",
        // A proposition the state settles *false*, so the noul operation is
        // not a column of `true`s that any component would get right.
        noul: ("Do payment changes need three approvals?", false),
        choice: (
            "How many approvals do payment changes need?",
            [
                ChoiceOption {
                    id: "one-approval",
                    description: "one approval is required",
                },
                ChoiceOption {
                    id: "two-approvals",
                    description: "two approvals are required",
                },
                ChoiceOption {
                    id: "three-approvals",
                    description: "three approvals are required",
                },
            ],
            1,
        ),
        score: ("How settled is the approval rule?", 2),
    },
    Topic {
        row_id: "zh-0001",
        lang: Lang::Zh,
        state: "部署密钥每九十天轮换一次，轮换策略已完全确定。ANSWER: ninety days",
        noul: ("轮换周期是九十天吗？", true),
        choice: (
            "最终确定的轮换周期是哪个？",
            [
                ChoiceOption {
                    id: "ninety-days",
                    description: "周期是九十天",
                },
                ChoiceOption {
                    id: "thirty-days",
                    description: "周期是三十天",
                },
                ChoiceOption {
                    id: "one-year",
                    description: "周期是一年",
                },
            ],
            0,
        ),
        score: ("轮换策略的确定程度有多高？", 2),
    },
    Topic {
        row_id: "zh-0002",
        lang: Lang::Zh,
        state: "夜间备份保留三十天，留存策略已完全确定。ANSWER: thirty days",
        noul: ("备份保留三十天吗？", true),
        choice: (
            "备份保留多久？",
            [
                ChoiceOption {
                    id: "seven-days",
                    description: "周期是七天",
                },
                ChoiceOption {
                    id: "thirty-days",
                    description: "周期是三十天",
                },
                ChoiceOption {
                    id: "ninety-days",
                    description: "周期是九十天",
                },
            ],
            1,
        ),
        score: ("留存策略的确定程度有多高？", 2),
    },
    Topic {
        row_id: "zh-0003",
        lang: Lang::Zh,
        state: "主区域是 eu-west-1，区域选择只是部分确定。ANSWER: eu-west-1",
        noul: ("主区域是 eu-west-1 吗？", true),
        choice: (
            "承载主流量的区域是哪个？",
            [
                ChoiceOption {
                    id: "us-east-1",
                    description: "区域是 us-east-1",
                },
                ChoiceOption {
                    id: "eu-west-1",
                    description: "区域是 eu-west-1",
                },
                ChoiceOption {
                    id: "ap-south-1",
                    description: "区域是 ap-south-1",
                },
            ],
            1,
        ),
        score: ("区域选择的确定程度有多高？", 1),
    },
    Topic {
        row_id: "zh-0004",
        lang: Lang::Zh,
        state: "支付变更需要两次批准，批准规则已完全确定。ANSWER: two approvals",
        noul: ("支付变更需要三次批准吗？", false),
        choice: (
            "支付变更需要几次批准？",
            [
                ChoiceOption {
                    id: "one-approval",
                    description: "需要一次批准",
                },
                ChoiceOption {
                    id: "two-approvals",
                    description: "需要两次批准",
                },
                ChoiceOption {
                    id: "three-approvals",
                    description: "需要三次批准",
                },
            ],
            1,
        ),
        score: ("批准规则的确定程度有多高？", 2),
    },
];

/// The rows of the shipped criteria set.
fn rows() -> Vec<CriterionRow> {
    let mut rows: Vec<CriterionRow> = TOPICS.iter().map(topic_row).collect();
    rows.push(unresolved_row(
        "en-0005-unresolved",
        Lang::En,
        "The rollback owner has not been named yet. UNRESOLVED: owner",
        "Has the rollback owner been named?",
    ));
    rows.push(unresolved_row(
        "zh-0005-unresolved",
        Lang::Zh,
        "回滚负责人尚未指定。UNRESOLVED: owner",
        "回滚负责人指定了吗？",
    ));
    rows
}

/// The row of one topic: its three questions, one per operation.
fn topic_row(topic: &Topic) -> CriterionRow {
    let (noul_instructions, noul_holds) = topic.noul;
    let (choice_instructions, choice_options, choice_answer) = &topic.choice;
    let (score_instructions, score_level) = topic.score;

    let noul_criteria = vec![
        Criterion {
            option_id: "true".to_string(),
            description: noul_description(topic.lang, true).to_string(),
        },
        Criterion {
            option_id: "false".to_string(),
            description: noul_description(topic.lang, false).to_string(),
        },
    ];
    let noul_gold = if noul_holds {
        Gold {
            label: "true".to_string(),
            probabilities: BTreeMap::from([("true".to_string(), 0.9), ("false".to_string(), 0.1)]),
        }
    } else {
        Gold {
            label: "false".to_string(),
            probabilities: BTreeMap::from([("true".to_string(), 0.1), ("false".to_string(), 0.9)]),
        }
    };

    let choice_criteria: Vec<Criterion> = choice_options
        .iter()
        .map(|option| Criterion {
            option_id: option.id.to_string(),
            description: option.description.to_string(),
        })
        .collect();
    let choice_gold = choice_gold(choice_options, *choice_answer);

    let score_criteria: Vec<Criterion> = (0..3)
        .map(|level| Criterion {
            option_id: level.to_string(),
            description: score_description(topic.lang, level).to_string(),
        })
        .collect();
    let score_gold = score_gold(score_level as usize);

    CriterionRow {
        row_id: topic.row_id.to_string(),
        state_hash: format!("state-{}", topic.row_id),
        lang: topic.lang,
        state: topic.state.to_string(),
        questions: vec![
            CriterionQuestion {
                question_id: format!("{}-noul", topic.row_id),
                op: Op::Noul,
                instructions: noul_instructions.to_string(),
                criteria: noul_criteria,
                gold: noul_gold,
            },
            CriterionQuestion {
                question_id: format!("{}-choice", topic.row_id),
                op: Op::Choice,
                instructions: choice_instructions.to_string(),
                criteria: choice_criteria,
                gold: choice_gold,
            },
            CriterionQuestion {
                question_id: format!("{}-score", topic.row_id),
                op: Op::Score,
                instructions: score_instructions.to_string(),
                criteria: score_criteria,
                gold: score_gold,
            },
        ],
    }
}

/// The row that must be abstained on.
///
/// It carries no answer — the state says the owner is not named — and the
/// frozen criteria say the right behaviour is to decline, so the abstain
/// rate has something to measure and the tiers can be compared on it.
fn unresolved_row(row_id: &str, lang: Lang, state: &str, instructions: &str) -> CriterionRow {
    CriterionRow {
        row_id: row_id.to_string(),
        state_hash: format!("state-{row_id}"),
        lang,
        state: state.to_string(),
        questions: vec![CriterionQuestion {
            question_id: format!("{row_id}-noul"),
            op: Op::Noul,
            instructions: instructions.to_string(),
            criteria: vec![
                Criterion {
                    option_id: "true".to_string(),
                    description: noul_description(lang, true).to_string(),
                },
                Criterion {
                    option_id: "false".to_string(),
                    description: noul_description(lang, false).to_string(),
                },
            ],
            gold: Gold {
                label: "false".to_string(),
                probabilities: BTreeMap::from([
                    ("true".to_string(), 0.1),
                    ("false".to_string(), 0.9),
                ]),
            },
        }],
    }
}

/// What the `true` and `false` options of a noul question mean.
fn noul_description(lang: Lang, holds: bool) -> &'static str {
    match (lang, holds) {
        (Lang::En, true) => "the statement holds",
        (Lang::En, false) => "the statement does not hold",
        (Lang::Zh, true) => "该陈述成立",
        (Lang::Zh, false) => "该陈述不成立",
    }
}

/// What the three levels of the settledness scale mean.
fn score_description(lang: Lang, level: u8) -> &'static str {
    match (lang, level) {
        (Lang::En, 0) => "not settled at all",
        (Lang::En, 1) => "partly settled",
        (Lang::En, 2) => "fully settled",
        (Lang::Zh, 0) => "完全没有定下来",
        (Lang::Zh, 1) => "部分定下来",
        (Lang::Zh, 2) => "完全定下来",
        _ => "not settled at all",
    }
}

/// The gold of a choice question: the right option carries most of the
/// mass, the two wrong ones share the rest.
fn choice_gold(options: &[ChoiceOption; 3], answer: usize) -> Gold {
    let mut probabilities = BTreeMap::new();
    let mut wrong = 0;
    for (index, option) in options.iter().enumerate() {
        let probability = if index == answer {
            0.8
        } else if wrong == 0 {
            wrong = 1;
            0.15
        } else {
            0.05
        };
        probabilities.insert(option.id.to_string(), probability);
    }
    Gold {
        label: options[answer].id.to_string(),
        probabilities,
    }
}

/// The gold of a score question: the level it settles on carries most of the
/// mass, and the mass falls off with the distance from it.
///
/// The three rows are the three distributions, written out rather than
/// computed: each of them has to sum to exactly one, and a computed one
/// would have to be checked to be sure it did.
fn score_gold(level: usize) -> Gold {
    const DISTRIBUTIONS: [[f64; 3]; 3] = [[0.7, 0.2, 0.1], [0.2, 0.6, 0.2], [0.1, 0.2, 0.7]];
    let distribution = DISTRIBUTIONS[level];
    let mut probabilities = BTreeMap::new();
    for (level_index, probability) in distribution.iter().enumerate() {
        probabilities.insert(level_index.to_string(), *probability);
    }
    Gold {
        label: level.to_string(),
        probabilities,
    }
}

/// The ladder this module ships with: four tiers, cheapest footprint first.
///
/// The footprints are the *declared* ones — the megabytes the tier's
/// components are said to occupy — and they are what the marginal-gain axis
/// divides by. The declared numbers are placeholders for a real profile;
/// what the framework measures is that the axis is the same for every tier
/// and that a step's gain is read against the megabytes it costs.
pub fn reference_ladder() -> Ladder {
    Ladder::frozen(
        "reference-ladder",
        vec![
            Tier {
                tier_id: "cloud".to_string(),
                label: "remote judge endpoint".to_string(),
                components: vec![Component {
                    component_id: "judge-cloud".to_string(),
                    kind: ComponentKind::Judge,
                    ram_mb: 0,
                    disk_mb: 0,
                }],
                budget: Budget {
                    ram_mb: 0,
                    disk_mb: 0,
                },
                adds: Vec::new(),
            },
            Tier {
                tier_id: "local-nano".to_string(),
                label: "small local judge".to_string(),
                components: vec![Component {
                    component_id: "judge-nano".to_string(),
                    kind: ComponentKind::Judge,
                    ram_mb: 40,
                    disk_mb: 40,
                }],
                budget: Budget {
                    ram_mb: 200,
                    disk_mb: 200,
                },
                adds: vec!["judge-nano".to_string()],
            },
            Tier {
                tier_id: "local-mini".to_string(),
                label: "larger local judge".to_string(),
                components: vec![Component {
                    component_id: "judge-mini".to_string(),
                    kind: ComponentKind::Judge,
                    ram_mb: 100,
                    disk_mb: 100,
                }],
                budget: Budget {
                    ram_mb: 300,
                    disk_mb: 300,
                },
                adds: vec!["judge-mini".to_string()],
            },
            Tier {
                tier_id: "local-mini-fp16".to_string(),
                label: "the same judge in a wider build".to_string(),
                components: vec![Component {
                    component_id: "judge-mini-fp16".to_string(),
                    kind: ComponentKind::Judge,
                    ram_mb: 180,
                    disk_mb: 180,
                }],
                budget: Budget {
                    ram_mb: 400,
                    disk_mb: 400,
                },
                adds: vec!["judge-mini-fp16".to_string()],
            },
        ],
        VisibilityPolicy::default(),
    )
    .expect("the shipped ladder is consistent")
}

/// The registry of mock components the shipped ladder runs against.
///
/// The four components differ in quality. `judge-mini-fp16` is deliberately
/// indistinguishable from `judge-mini`: it costs 80 MB more and answers
/// exactly the same, which is what gives the protocol's drop rule something
/// to act on.
pub fn mock_registry(criteria: &Criteria) -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    registry.register(Arc::new(MockComponent::new(
        "judge-cloud",
        MockBehaviour {
            skill: 0.90,
            abstain_rate: 0.0,
            known_confidence: 0.94,
            guessed_confidence: 0.30,
        },
        criteria,
    )));
    registry.register(Arc::new(MockComponent::new(
        "judge-nano",
        MockBehaviour {
            skill: 0.55,
            abstain_rate: 0.10,
            known_confidence: 0.70,
            guessed_confidence: 0.30,
        },
        criteria,
    )));
    registry.register(Arc::new(MockComponent::new(
        "judge-mini",
        MockBehaviour {
            skill: 0.78,
            abstain_rate: 0.05,
            known_confidence: 0.88,
            guessed_confidence: 0.30,
        },
        criteria,
    )));
    registry.register(Arc::new(MockComponent::new(
        "judge-mini-fp16",
        MockBehaviour {
            skill: 0.78,
            abstain_rate: 0.05,
            known_confidence: 0.88,
            guessed_confidence: 0.30,
        },
        criteria,
    )));
    registry
}

/// How a mock component behaves.
///
/// The fields are the component's quality, not its name: two components with
/// the same behaviour answer identically, however they are called.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MockBehaviour {
    /// The share of questions the component answers correctly.
    pub skill: f64,
    /// The share of questions it abstains on.
    pub abstain_rate: f64,
    /// The confidence it reports when it knows the answer.
    pub known_confidence: f64,
    /// The confidence it reports when it guesses.
    pub guessed_confidence: f64,
}

impl MockBehaviour {
    /// Whether a question of this difficulty is one the component knows.
    ///
    /// The threshold is the behaviour's and the draw is the question's, so a
    /// component with a higher skill knows a **superset** of what a
    /// lower-skill component knows.
    fn knows(&self, difficulty: f64) -> bool {
        difficulty < self.skill
    }

    /// Whether a question the component is this reluctant about is one it
    /// declines.
    ///
    /// A component with a lower abstention rate therefore answers a superset
    /// of what a more reluctant one answers.
    fn declines(&self, reluctance: f64) -> bool {
        reluctance < self.abstain_rate
    }
}

/// How hard a question is, in `[0, 1)`: the draw a component's skill is
/// compared against.
fn difficulty(question_id: &str) -> f64 {
    unit_hash(&format!("difficulty|{question_id}"))
}

/// How reluctant a component is about a question, in `[0, 1)`: the draw its
/// abstention rate is compared against.
fn reluctance(question_id: &str) -> f64 {
    unit_hash(&format!("reluctance|{question_id}"))
}

/// A deterministic stand-in for a judgment component.
///
/// It holds the gold answers, because it stands in for a model trained on
/// them. On each question it decides — from the question's id and its own
/// behaviour, never from a call counter — whether it knows the answer and
/// whether it abstains, and then answers.
pub struct MockComponent {
    component_id: String,
    behaviour: MockBehaviour,
    /// The gold label of every question, by question id.
    gold: BTreeMap<String, String>,
}

impl MockComponent {
    /// A mock component with this id, this behaviour and the gold answers
    /// of `criteria`.
    pub fn new(
        component_id: impl Into<String>,
        behaviour: MockBehaviour,
        criteria: &Criteria,
    ) -> Self {
        let mut gold = BTreeMap::new();
        for row in &criteria.rows {
            for question in &row.questions {
                gold.insert(question.question_id.clone(), question.gold.label.clone());
            }
        }
        Self {
            component_id: component_id.into(),
            behaviour,
            gold,
        }
    }
}

#[async_trait]
impl ComponentProvider for MockComponent {
    fn component_id(&self) -> &str {
        &self.component_id
    }

    async fn answer(&self, request: &JudgeRequest) -> Result<JudgeAnswer, String> {
        if !request.state.contains(ANSWER_MARKER) {
            // A state with no answer in it cannot be answered, and saying so
            // is the right behaviour rather than a guess.
            return Ok(abstained());
        }
        if self.behaviour.declines(reluctance(&request.question_id)) {
            return Ok(abstained());
        }
        let gold = self.gold.get(&request.question_id).ok_or_else(|| {
            format!(
                "mock component {} has no gold answer for question {}",
                self.component_id, request.question_id
            )
        })?;
        let knows = self.behaviour.knows(difficulty(&request.question_id));
        let (selected, confidence) = if knows {
            (gold.clone(), self.behaviour.known_confidence)
        } else {
            (self.guess(request), self.behaviour.guessed_confidence)
        };
        let mut probabilities = BTreeMap::new();
        if request.op != Op::Noul {
            for criterion in &request.criteria {
                let weight = if criterion.option_id == selected {
                    confidence
                } else {
                    0.0
                };
                probabilities.insert(criterion.option_id.clone(), weight);
            }
        }
        // A noul answer is a probability, so its confidence cannot be
        // reported separately without the two disagreeing: the margin from
        // one half *is* the confidence. The side of the answer carries the
        // sign, the margin carries the strength.
        let noul = match request.op {
            Op::Noul => Some(if selected == "true" {
                0.5 + confidence / 2.0
            } else {
                0.5 - confidence / 2.0
            }),
            _ => None,
        };
        Ok(JudgeAnswer {
            choice: match request.op {
                Op::Choice => Some(selected.clone()),
                _ => None,
            },
            noul,
            score: match request.op {
                Op::Score => request
                    .criteria
                    .iter()
                    .position(|criterion| criterion.option_id == selected)
                    .map(|index| index as f64),
                _ => None,
            },
            probabilities,
            confidence: match request.op {
                Op::Noul => None,
                _ => Some(confidence),
            },
            abstain: false,
        })
    }
}

impl MockComponent {
    /// The guess: an option drawn from the list **as presented**.
    ///
    /// Drawing from the presented list is what makes the option-order probe
    /// bite: the same question with the options reversed draws a different
    /// option, which the probe reads as a flip.
    ///
    /// The draw is keyed by the question and the presented list, not by the
    /// component: two components that are both guessing at a question guess
    /// the same option, so a stronger judge is never wrong where a weaker
    /// one was right.
    fn guess(&self, request: &JudgeRequest) -> String {
        let options = &request.criteria;
        if options.is_empty() {
            return String::new();
        }
        let presented: Vec<&str> = options
            .iter()
            .map(|criterion| criterion.option_id.as_str())
            .collect();
        let draw = unit_hash(&format!(
            "guess|{}|{}",
            request.question_id,
            presented.join(",")
        ));
        let index = (draw * options.len() as f64).floor() as usize;
        options[index.min(options.len() - 1)].option_id.clone()
    }
}

/// An answer that declines.
fn abstained() -> JudgeAnswer {
    JudgeAnswer {
        choice: None,
        noul: None,
        score: None,
        probabilities: BTreeMap::new(),
        confidence: None,
        abstain: true,
    }
}

/// A deterministic draw in `[0, 1)`, keyed by `label`.
///
/// The hash is the repository's own, so the value is the same on every
/// machine and in every run: a mock that depended on a seeded RNG's call
/// order would answer differently depending on how the ladder walked it.
///
/// The whole 64 bits of the digest are read, so the draw is spread over the
/// unit interval instead of being squeezed into its first 32 bits — a draw
/// taken from a narrow slice of the range would collapse every threshold
/// comparison onto the same end of it.
fn unit_hash(label: &str) -> f64 {
    let digest = crate::core::text::compute_mdhash_id(label, "");
    let head = digest.get(..16).unwrap_or("");
    let value = u64::from_str_radix(head, 16).unwrap_or(0);
    value as f64 / u64::MAX as f64
}
