//! Write-side judgment step (R4.T4 deliverable).
//!
//! [`judge_write_candidates`] is a pure function — no store I/O
//! besides the [`JudgeClient`] calls. The governance write gateway
//! (harness integration C01/C06) owns derivation, value screening,
//! near-neighbor lookup and commit; it calls this function between
//! screening and `FactStore::put`. Judge failure aborts the turn
//! commit (watermark stays).
//!
//! Naming: this is the capability surface, so everything here is
//! `Judge`/`Fact` vocabulary — no `jev` identifiers (the jev wire
//! names live only in the backend's private wire module).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::core::types::{FactId, ScopeKey};
use crate::llm::judge::{
    ClassifyClass, ClassifyItem, ClassifyRequest, ClassifyVerdict, CompareRelation, CompareRequest,
    ExtractField, ExtractRequest, ExtractResult, JudgeClient, JudgeError, JudgeResult, Verdict,
};

/// One locally derived fact candidate (the write-side judgment
/// input). Derivation rule (initial): one closed turn = one
/// candidate; the turn text is the fact text. Topic splitting is
/// a later refinement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactCandidate {
    pub text: String,
    pub scope: ScopeKey,
    pub source_refs: Vec<String>,
}

/// One already-written near neighbor of a candidate (the compare
/// input). The gateway fetches these from `FactVectorStore` (same
/// scope, ANN top-10, cosine ≥ `compare_min_cosine`) before
/// calling [`judge_write_candidates`] — the function itself stays
/// store-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparePassage {
    pub text: String,
    pub fact_id: FactId,
}

/// Per-near-neighbor comparison outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareResult {
    pub fact_id: FactId,
    pub relation: CompareRelation,
    pub confidence: f64,
}

/// A candidate after judgment: its type, its dedup/conflict
/// outcomes against near neighbors, and its extracted anchors.
#[derive(Debug, Clone)]
pub struct JudgedCandidate {
    pub candidate: FactCandidate,
    pub classify: Verdict,
    pub compare: Vec<CompareResult>,
    pub extract: BTreeMap<String, ExtractResult>,
}

/// The fixed classify catalog for write-side typing — one class
/// per documented `Fact.kind` value (semantic | episodic |
/// procedural), each with a one-sentence criterion.
pub static WRITE_CLASSIFY_CATALOG: LazyLock<Vec<ClassifyClass>> = LazyLock::new(|| {
    vec![
        ClassifyClass {
            id: "semantic".into(),
            description: "A reusable knowledge fact that stays true beyond this session: an entity, relationship, preference, or constraint.".into(),
        },
        ClassifyClass {
            id: "episodic".into(),
            description: "An event or session-bound fragment — what happened, when, who did what — not reusable outside its session.".into(),
        },
        ClassifyClass {
            id: "procedural".into(),
            description: "An operation step or how-to instruction: a task anchor or workflow step to follow.".into(),
        },
    ]
});

/// The fixed extract catalog for write-side anchors (initial set).
/// Extensible: adding a field here is a constant change, not a
/// trait change. Extracted values are appended to the fact text's
/// search copy (lexical-arm exact recall) — no schema column.
pub static WRITE_EXTRACT_FIELDS: LazyLock<Vec<ExtractField>> = LazyLock::new(|| {
    vec![
        ExtractField {
            id: "error_code".into(),
            pattern: r"\b[A-Z][A-Z0-9]*(?:[-_][A-Z0-9]+)+\b".into(),
            description:
                "An error or status code mentioned in the turn (e.g. ERR-1234, EAI_AGAIN).".into(),
        },
        ExtractField {
            id: "path".into(),
            pattern: r#"(?:/(?:[\w.-]+/)+[\w.-]+)|(?:\b[A-Za-z]:\\[^\s"']+)"#.into(),
            description: "A file or directory path mentioned in the turn.".into(),
        },
        ExtractField {
            id: "task_id".into(),
            pattern: r"\b[Tt]ask[-_:#]?\d+\b".into(),
            description: "A task identifier mentioned in the turn.".into(),
        },
    ]
});

/// Batch limit for classify/extract requests (超限分批排队，不截断).
const WRITE_BATCH_LIMIT: usize = 64;

/// Judge write-side candidates: ② classify typing, ③ compare
/// dedup/conflict against near neighbors, ④ anchor extraction.
///
/// `near_neighbors[i]` is the near-neighbor list of
/// `candidates[i]` (the gateway pre-fetches them; one list per
/// candidate, possibly empty). Any judge failure aborts the whole
/// batch — the caller stops the turn commit.
pub async fn judge_write_candidates(
    candidates: Vec<FactCandidate>,
    near_neighbors: Vec<Vec<ComparePassage>>,
    client: &dyn JudgeClient,
) -> JudgeResult<Vec<JudgedCandidate>> {
    assert_eq!(
        near_neighbors.len(),
        candidates.len(),
        "one near-neighbor list per candidate"
    );

    // ② classify — batched at WRITE_BATCH_LIMIT, results back-filled
    // by item id (c{index}); a missing id is a malformed response.
    let mut classify: Vec<Option<ClassifyVerdict>> = vec![None; candidates.len()];
    for chunk_start in (0..candidates.len()).step_by(WRITE_BATCH_LIMIT) {
        let chunk_end = (chunk_start + WRITE_BATCH_LIMIT).min(candidates.len());
        let items: Vec<ClassifyItem> = (chunk_start..chunk_end)
            .map(|i| ClassifyItem {
                id: format!("c{i}"),
                text: candidates[i].text.clone(),
            })
            .collect();
        let request = ClassifyRequest {
            items,
            catalog: WRITE_CLASSIFY_CATALOG.clone(),
            purpose: Some("write typing".into()),
        };
        let response = client.classify(&request).await?;
        for verdict in response.verdicts {
            let index = verdict
                .item_id
                .strip_prefix('c')
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|i| *i < candidates.len())
                .ok_or(JudgeError::InvalidResponse)?;
            classify[index] = Some(verdict);
        }
    }
    let classify: Vec<ClassifyVerdict> = classify
        .into_iter()
        .enumerate()
        .map(|(i, v)| v.ok_or(JudgeError::InvalidResponse).map(|v| (i, v)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(_, v)| v)
        .collect();

    // ③ compare + ④ extract, per candidate.
    let mut judged = Vec::with_capacity(candidates.len());
    for (i, candidate) in candidates.iter().enumerate() {
        let mut compare = Vec::with_capacity(near_neighbors[i].len());
        for passage in &near_neighbors[i] {
            let request = CompareRequest {
                passage_a: candidate.text.clone(),
                passage_b: passage.text.clone(),
                aspects: Vec::new(),
            };
            let response = client.compare(&request).await?;
            compare.push(CompareResult {
                fact_id: passage.fact_id.clone(),
                relation: response.relation,
                confidence: response.confidence,
            });
        }

        let request = ExtractRequest {
            document: candidate.text.clone(),
            fields: WRITE_EXTRACT_FIELDS.clone(),
        };
        let response = client.extract(&request).await?;
        let extract: BTreeMap<String, ExtractResult> = response
            .fields
            .into_iter()
            .map(|r| (r.field_id.clone(), r))
            .collect();

        judged.push(JudgedCandidate {
            candidate: candidate.clone(),
            classify: classify[i].verdict,
            compare,
            extract,
        });
    }

    Ok(judged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::TrustState;
    use crate::llm::judge::{ClassifyResponse, CompareResponse, ExtractResponse, ExtractStatus};
    use crate::llm::judge_mock::{JudgeMock, JudgeOp, ScriptedReply};

    fn candidate(text: &str) -> FactCandidate {
        FactCandidate {
            text: text.into(),
            scope: ScopeKey {
                workspace_id: "w".into(),
                session_id: Some("s".into()),
                task_id: None,
            },
            source_refs: vec!["turn-1".into()],
        }
    }

    #[tokio::test]
    async fn classifies_compares_and_extracts_in_order() {
        let mock = JudgeMock::new(
            "judge-mock",
            vec![
                ScriptedReply {
                    op: JudgeOp::Classify,
                    reply: crate::llm::judge_mock::JudgeReply::Classify(ClassifyResponse {
                        verdicts: vec![
                            ClassifyVerdict {
                                item_id: "c0".into(),
                                class_id: "semantic".into(),
                                verdict: Verdict::Yes,
                                probability: 0.9,
                            },
                            ClassifyVerdict {
                                item_id: "c1".into(),
                                class_id: "episodic".into(),
                                verdict: Verdict::Yes,
                                probability: 0.7,
                            },
                        ],
                        auto: true,
                    }),
                },
                ScriptedReply {
                    op: JudgeOp::Compare,
                    reply: crate::llm::judge_mock::JudgeReply::Compare(CompareResponse {
                        relation: CompareRelation::SameFact,
                        confidence: 0.95,
                        aspects: vec![],
                    }),
                },
                ScriptedReply {
                    op: JudgeOp::Extract,
                    reply: crate::llm::judge_mock::JudgeReply::Extract(ExtractResponse {
                        fields: vec![ExtractResult {
                            field_id: "error_code".into(),
                            value: Some("ERR-1234".into()),
                            status: ExtractStatus::Auto,
                        }],
                    }),
                },
                ScriptedReply {
                    op: JudgeOp::Extract,
                    reply: crate::llm::judge_mock::JudgeReply::Extract(ExtractResponse {
                        fields: vec![],
                    }),
                },
            ],
        );

        let candidates = vec![
            candidate("ACME acquired Beta Labs; see ERR-1234."),
            candidate("User ran the migration at 10:00."),
        ];
        let neighbor = ComparePassage {
            text: "ACME bought Beta Labs.".into(),
            fact_id: FactId("fact-1".into()),
        };
        let judged = judge_write_candidates(candidates, vec![vec![neighbor], vec![]], &mock)
            .await
            .expect("scripted judgment");

        assert_eq!(judged.len(), 2);
        assert_eq!(judged[0].classify, Verdict::Yes);
        assert_eq!(judged[1].classify, Verdict::Yes);
        // candidate 0 compared against its one neighbor; candidate 1 had none
        assert_eq!(judged[0].compare.len(), 1);
        assert_eq!(judged[0].compare[0].relation, CompareRelation::SameFact);
        assert_eq!(judged[0].compare[0].fact_id, FactId("fact-1".into()));
        assert!(judged[1].compare.is_empty());
        // extract results land in the per-candidate map
        assert_eq!(
            judged[0]
                .extract
                .get("error_code")
                .and_then(|r| r.value.as_deref()),
            Some("ERR-1234")
        );
        // call order: classify, compare, extract, extract
        assert_eq!(mock.calls(), 4);
    }

    #[tokio::test]
    async fn missing_classify_verdict_is_invalid_response() {
        let mock = JudgeMock::new(
            "judge-mock",
            vec![ScriptedReply {
                op: JudgeOp::Classify,
                reply: crate::llm::judge_mock::JudgeReply::Classify(ClassifyResponse {
                    verdicts: vec![], // c0/c1 both missing
                    auto: false,
                }),
            }],
        );
        let result = judge_write_candidates(vec![candidate("a")], vec![vec![]], &mock).await;
        assert!(matches!(result, Err(JudgeError::InvalidResponse)));
    }

    #[tokio::test]
    async fn unknown_classify_item_id_is_invalid_response() {
        let mock = JudgeMock::new(
            "judge-mock",
            vec![ScriptedReply {
                op: JudgeOp::Classify,
                reply: crate::llm::judge_mock::JudgeReply::Classify(ClassifyResponse {
                    verdicts: vec![ClassifyVerdict {
                        item_id: "c99".into(), // out of range
                        class_id: "semantic".into(),
                        verdict: Verdict::Yes,
                        probability: 0.9,
                    }],
                    auto: true,
                }),
            }],
        );
        let result = judge_write_candidates(vec![candidate("a")], vec![vec![]], &mock).await;
        assert!(matches!(result, Err(JudgeError::InvalidResponse)));
    }

    #[tokio::test]
    async fn judge_failure_aborts_the_batch() {
        let mock = JudgeMock::new(
            "judge-mock",
            vec![ScriptedReply {
                op: JudgeOp::Classify,
                reply: crate::llm::judge_mock::JudgeReply::Fail(JudgeError::Backend(
                    crate::llm::envelope::ProviderError::Unavailable {
                        op: crate::llm::envelope::Op::Judge,
                    },
                )),
            }],
        );
        let result = judge_write_candidates(vec![candidate("a")], vec![vec![]], &mock).await;
        assert!(matches!(result, Err(JudgeError::Backend(_))));
    }

    #[tokio::test]
    async fn classify_batches_beyond_the_batch_limit() {
        // 65 candidates → two classify calls (64 + 1), then per-candidate
        // compare (none) and extract (one per candidate). Script: 2 classify
        // replies + 65 extract replies.
        let mut script = vec![
            ScriptedReply {
                op: JudgeOp::Classify,
                reply: crate::llm::judge_mock::JudgeReply::Classify(ClassifyResponse {
                    verdicts: (0..64)
                        .map(|i| ClassifyVerdict {
                            item_id: format!("c{i}"),
                            class_id: "semantic".into(),
                            verdict: Verdict::Yes,
                            probability: 0.9,
                        })
                        .collect(),
                    auto: true,
                }),
            },
            ScriptedReply {
                op: JudgeOp::Classify,
                reply: crate::llm::judge_mock::JudgeReply::Classify(ClassifyResponse {
                    verdicts: vec![ClassifyVerdict {
                        item_id: "c64".into(),
                        class_id: "semantic".into(),
                        verdict: Verdict::Yes,
                        probability: 0.9,
                    }],
                    auto: true,
                }),
            },
        ];
        for _ in 0..65 {
            script.push(ScriptedReply {
                op: JudgeOp::Extract,
                reply: crate::llm::judge_mock::JudgeReply::Extract(ExtractResponse {
                    fields: vec![],
                }),
            });
        }
        let mock = JudgeMock::new("judge-mock", script);
        let candidates: Vec<FactCandidate> =
            (0..65).map(|i| candidate(&format!("fact {i}"))).collect();
        let judged = judge_write_candidates(candidates, vec![vec![]; 65], &mock)
            .await
            .expect("scripted judgment");
        assert_eq!(judged.len(), 65);
        // 2 classify + 65 extract = 67 calls, zero compare
        assert_eq!(mock.calls(), 67);
    }

    #[test]
    fn catalogs_cover_the_fact_kinds_and_extract_fields() {
        let kinds: Vec<&str> = WRITE_CLASSIFY_CATALOG
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(kinds, vec!["semantic", "episodic", "procedural"]);
        let fields: Vec<&str> = WRITE_EXTRACT_FIELDS.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(fields, vec!["error_code", "path", "task_id"]);
        // patterns are valid regexes (the backend compiles them client-side)
        for field in WRITE_EXTRACT_FIELDS.iter() {
            regex::Regex::new(&field.pattern).expect("extract pattern is valid");
        }
    }

    #[test]
    fn fact_candidate_carries_the_turn_shape() {
        let candidate = candidate("text");
        assert_eq!(candidate.source_refs, vec!["turn-1".to_string()]);
        // TrustState stays on the written Fact, not the candidate
        let _ = TrustState::Candidate;
    }
}
