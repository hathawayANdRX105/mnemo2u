//! Scripted judgment mock (R4.T1): fixture replies in call order,
//! with [`JudgeReply::Fail`] fault injection (500/timeout/invalid
//! response) sharing the same shape as real replies.
//!
//! Script exhaustion and op/script mismatches panic — they are test
//! bugs (the fixture did not expect this call), not judgment
//! failures, and failing loudly beats silently succeeding.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::llm::judge::{
    ClassifyRequest, ClassifyResponse, CompareRequest, CompareResponse, ExistsVerdict,
    ExtractRequest, ExtractResponse, FindRequest, FindResponse, JudgeClient, JudgeError,
    JudgeResult, RerankCandidate, RerankHit, RerankRequest, RerankResponse, Verdict,
};

/// The five judgment operations (script routing key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeOp {
    Classify,
    Compare,
    Extract,
    Rerank,
    Find,
}

/// A scripted reply for one operation, or an injected failure.
#[derive(Debug, Clone)]
pub enum JudgeReply {
    Classify(ClassifyResponse),
    Compare(CompareResponse),
    Extract(ExtractResponse),
    Rerank(RerankResponse),
    Find(FindResponse),
    Fail(JudgeError),
}

/// One scripted step: which operation it answers, and the reply.
#[derive(Debug, Clone)]
pub struct ScriptedReply {
    pub op: JudgeOp,
    pub reply: JudgeReply,
}

/// Every request the mock received, in call order (assertion aid).
#[derive(Debug, Clone)]
pub enum JudgeCall {
    Classify(ClassifyRequest),
    Compare(CompareRequest),
    Extract(ExtractRequest),
    Rerank(RerankRequest),
    Find(FindRequest),
}

/// Scripted [`JudgeClient`]: consumes `script` in call order.
pub struct JudgeMock {
    model: String,
    script: Mutex<VecDeque<ScriptedReply>>,
    calls: AtomicUsize,
    sent: Mutex<Vec<JudgeCall>>,
}

impl JudgeMock {
    pub fn new(model: impl Into<String>, script: Vec<ScriptedReply>) -> Self {
        Self {
            model: model.into(),
            script: Mutex::new(script.into()),
            calls: AtomicUsize::new(0),
            sent: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Every request received so far, in call order.
    pub fn sent_calls(&self) -> Vec<JudgeCall> {
        self.sent.lock().expect("judge mock lock").clone()
    }

    fn next(&self, op: JudgeOp, call: JudgeCall) -> JudgeReply {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.sent.lock().expect("judge mock lock").push(call);
        let scripted = self
            .script
            .lock()
            .expect("judge mock lock")
            .pop_front()
            .unwrap_or_else(|| panic!("judge mock script exhausted: no scripted reply for {op:?}"));
        assert_eq!(
            scripted.op, op,
            "judge mock script/op mismatch: test expected {:?}, call was {op:?}",
            scripted.op
        );
        scripted.reply
    }
}

/// Convenience constructors for the common scripted shapes.
impl JudgeMock {
    /// A classify reply where every item lands in `class_id` with
    /// `verdict` and the given probability.
    pub fn classify_all(
        item_ids: &[&str],
        class_id: &str,
        verdict: Verdict,
        probability: f64,
    ) -> ScriptedReply {
        ScriptedReply {
            op: JudgeOp::Classify,
            reply: JudgeReply::Classify(ClassifyResponse {
                verdicts: item_ids
                    .iter()
                    .map(|id| crate::llm::judge::ClassifyVerdict {
                        item_id: id.to_string(),
                        class_id: class_id.into(),
                        verdict,
                        probability,
                    })
                    .collect(),
                auto: !matches!(verdict, Verdict::Review),
            }),
        }
    }

    /// A rerank reply ranking `id -> relevance` pairs in order.
    pub fn rerank_ranked(pairs: &[(&str, f64)]) -> ScriptedReply {
        ScriptedReply {
            op: JudgeOp::Rerank,
            reply: JudgeReply::Rerank(RerankResponse {
                ranked: pairs
                    .iter()
                    .map(|(id, relevance)| RerankHit {
                        id: id.to_string(),
                        relevance: *relevance,
                    })
                    .collect(),
            }),
        }
    }

    /// A find reply with the given best hit and existence verdict.
    pub fn find(best: Option<(&str, f64)>, exists_verdict: ExistsVerdict) -> ScriptedReply {
        ScriptedReply {
            op: JudgeOp::Find,
            reply: JudgeReply::Find(FindResponse {
                best: best.map(|(id, relevance)| RerankHit {
                    id: id.to_string(),
                    relevance,
                }),
                exists_verdict,
            }),
        }
    }
}

/// A single candidate for tests (`RerankCandidate` alias keeps call
/// sites short; the wire type is unchanged).
pub fn candidate(id: &str, text: &str) -> RerankCandidate {
    RerankCandidate {
        id: id.into(),
        text: text.into(),
    }
}

#[async_trait]
impl JudgeClient for JudgeMock {
    fn model_id(&self) -> String {
        self.model.clone()
    }

    async fn classify(&self, req: &ClassifyRequest) -> JudgeResult<ClassifyResponse> {
        match self.next(JudgeOp::Classify, JudgeCall::Classify(req.clone())) {
            JudgeReply::Classify(reply) => Ok(reply),
            JudgeReply::Fail(error) => Err(error),
            _ => Err(JudgeError::InvalidResponse),
        }
    }

    async fn compare(&self, req: &CompareRequest) -> JudgeResult<CompareResponse> {
        match self.next(JudgeOp::Compare, JudgeCall::Compare(req.clone())) {
            JudgeReply::Compare(reply) => Ok(reply),
            JudgeReply::Fail(error) => Err(error),
            _ => Err(JudgeError::InvalidResponse),
        }
    }

    async fn extract(&self, req: &ExtractRequest) -> JudgeResult<ExtractResponse> {
        match self.next(JudgeOp::Extract, JudgeCall::Extract(req.clone())) {
            JudgeReply::Extract(reply) => Ok(reply),
            JudgeReply::Fail(error) => Err(error),
            _ => Err(JudgeError::InvalidResponse),
        }
    }

    async fn rerank(&self, req: &RerankRequest) -> JudgeResult<RerankResponse> {
        match self.next(JudgeOp::Rerank, JudgeCall::Rerank(req.clone())) {
            JudgeReply::Rerank(reply) => Ok(reply),
            JudgeReply::Fail(error) => Err(error),
            _ => Err(JudgeError::InvalidResponse),
        }
    }

    async fn find(&self, req: &FindRequest) -> JudgeResult<FindResponse> {
        match self.next(JudgeOp::Find, JudgeCall::Find(req.clone())) {
            JudgeReply::Find(reply) => Ok(reply),
            JudgeReply::Fail(error) => Err(error),
            _ => Err(JudgeError::InvalidResponse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::judge::{ClassifyClass, ClassifyItem, ClassifyVerdict, CompareRelation};

    #[tokio::test]
    async fn script_consumes_in_call_order() {
        let mock = JudgeMock::new(
            "judge-mock",
            vec![
                JudgeMock::classify_all(&["i0", "i1"], "semantic", Verdict::Yes, 0.9),
                JudgeMock::rerank_ranked(&[("c2", 0.8), ("c0", 0.3)]),
                JudgeMock::find(Some(("c2", 0.8)), ExistsVerdict::Answered),
            ],
        );

        let classified = mock
            .classify(&ClassifyRequest {
                items: vec![ClassifyItem {
                    id: "i0".into(),
                    text: "fact one".into(),
                }],
                catalog: vec![ClassifyClass {
                    id: "semantic".into(),
                    description: "reusable knowledge".into(),
                }],
                purpose: Some("write typing".into()),
            })
            .await
            .expect("scripted classify");
        assert_eq!(classified.verdicts.len(), 2);
        assert_eq!(
            classified.verdicts[0],
            ClassifyVerdict {
                item_id: "i0".into(),
                class_id: "semantic".into(),
                verdict: Verdict::Yes,
                probability: 0.9,
            }
        );
        assert!(classified.auto);

        let ranked = mock
            .rerank(&RerankRequest {
                query: "q".into(),
                candidates: vec![candidate("c0", "a"), candidate("c2", "b")],
            })
            .await
            .expect("scripted rerank");
        assert_eq!(ranked.ranked[0].id, "c2");

        let found = mock
            .find(&FindRequest {
                query: "q".into(),
                candidates: vec![candidate("c2", "b")],
            })
            .await
            .expect("scripted find");
        assert_eq!(found.exists_verdict, ExistsVerdict::Answered);
        assert_eq!(found.best.expect("best").id, "c2");

        assert_eq!(mock.calls(), 3);
        assert_eq!(mock.sent_calls().len(), 3);
    }

    #[tokio::test]
    async fn fail_injection_returns_the_injected_error() {
        let mock = JudgeMock::new(
            "judge-mock",
            vec![ScriptedReply {
                op: JudgeOp::Rerank,
                reply: JudgeReply::Fail(JudgeError::Backend(
                    crate::llm::envelope::ProviderError::Unavailable {
                        op: crate::llm::envelope::Op::Judge,
                    },
                )),
            }],
        );
        let result = mock
            .rerank(&RerankRequest {
                query: "q".into(),
                candidates: vec![],
            })
            .await;
        assert!(matches!(
            result,
            Err(JudgeError::Backend(
                crate::llm::envelope::ProviderError::Unavailable { .. }
            ))
        ));
    }

    #[tokio::test]
    #[should_panic(expected = "script exhausted")]
    async fn exhausting_the_script_panics() {
        let mock = JudgeMock::new("judge-mock", vec![]);
        let _ = mock
            .rerank(&RerankRequest {
                query: "q".into(),
                candidates: vec![],
            })
            .await;
    }

    #[tokio::test]
    #[should_panic(expected = "op mismatch")]
    async fn op_script_mismatch_panics() {
        let mock = JudgeMock::new("judge-mock", vec![JudgeMock::rerank_ranked(&[])]);
        let _ = mock
            .classify(&ClassifyRequest {
                items: vec![],
                catalog: vec![],
                purpose: None,
            })
            .await;
    }

    #[tokio::test]
    async fn compare_relation_round_trips() {
        // Sanity: the DTO enums the mock carries are the contract types.
        let relation = CompareRelation::SameFact;
        assert_eq!(relation, CompareRelation::SameFact);
    }
}
