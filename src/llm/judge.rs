//! Judgment capability surface (R4.T1).
//!
//! The narrow [`JudgeClient`] trait carries the five judgment
//! operations with their DTOs. The trait is capability-facing, not
//! protocol-facing: backends (jev-compatible endpoint, mocks, the
//! disabled null object) all implement it, and the write/read wiring
//! never names a backend.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Unresolved-aware verdict. `Review` = unresolved — the consumer must
/// distinguish it from `Yes`/`No` (AC3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Yes,
    No,
    Review,
}

/// Passage relation for dedup/conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareRelation {
    SameFact,
    Contradicts,
    DifferentFacts,
}

/// Field extraction outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtractStatus {
    Auto,
    Review,
    NotFound,
}

/// Answer-existence verdict for find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExistsVerdict {
    Answered,
    Partial,
    Absent,
}

// ---- classify ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassifyItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassifyClass {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassifyRequest {
    pub items: Vec<ClassifyItem>,
    pub catalog: Vec<ClassifyClass>,
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassifyVerdict {
    pub item_id: String,
    pub class_id: String,
    pub verdict: Verdict,
    pub probability: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassifyResponse {
    pub verdicts: Vec<ClassifyVerdict>,
    pub auto: bool,
}

// ---- compare ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AspectVerdict {
    pub aspect: String,
    pub relation: CompareRelation,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompareRequest {
    pub passage_a: String,
    pub passage_b: String,
    pub aspects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareResponse {
    pub relation: CompareRelation,
    pub confidence: f64,
    pub aspects: Vec<AspectVerdict>,
}

// ---- extract ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractField {
    pub id: String,
    pub pattern: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractRequest {
    pub document: String,
    pub fields: Vec<ExtractField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractResult {
    pub field_id: String,
    pub value: Option<String>,
    pub status: ExtractStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractResponse {
    pub fields: Vec<ExtractResult>,
}

// ---- rerank / find ---------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RerankCandidate {
    pub id: String,
    pub text: String,
}

/// ≤250 candidates; callers queue beyond the cap (never truncate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RerankRequest {
    pub query: String,
    pub candidates: Vec<RerankCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RerankHit {
    pub id: String,
    pub relevance: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RerankResponse {
    pub ranked: Vec<RerankHit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindRequest {
    pub query: String,
    pub candidates: Vec<RerankCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindResponse {
    pub best: Option<RerankHit>,
    pub exists_verdict: ExistsVerdict,
}

// ---- errors ----------------------------------------------------------

/// Judgment-side error: disabled null object, backend failure, or a
/// malformed response (fix the call, not the retry).
#[derive(Debug, Clone, thiserror::Error)]
pub enum JudgeError {
    #[error("judgment client disabled")]
    Disabled,
    #[error(transparent)]
    Backend(crate::llm::envelope::ProviderError),
    #[error("invalid judgment response")]
    InvalidResponse,
}

pub type JudgeResult<T> = Result<T, JudgeError>;

/// The judgment capability. Five operations, one DTO set, backend-agnostic.
#[async_trait]
pub trait JudgeClient: Send + Sync {
    /// Model identity — part of the cache key and the metering row.
    fn model_id(&self) -> String;
    async fn classify(&self, req: &ClassifyRequest) -> JudgeResult<ClassifyResponse>;
    async fn compare(&self, req: &CompareRequest) -> JudgeResult<CompareResponse>;
    async fn extract(&self, req: &ExtractRequest) -> JudgeResult<ExtractResponse>;
    async fn rerank(&self, req: &RerankRequest) -> JudgeResult<RerankResponse>;
    async fn find(&self, req: &FindRequest) -> JudgeResult<FindResponse>;
}

/// Null object: the default injection. Every method returns
/// `Err(JudgeError::Disabled)` with zero network I/O — the egress gate's
/// second line of defense (the gate denies before the call; this makes
/// the denial explicit at the capability surface).
pub struct DisabledJudge;

#[async_trait]
impl JudgeClient for DisabledJudge {
    fn model_id(&self) -> String {
        "disabled".into()
    }
    async fn classify(&self, _req: &ClassifyRequest) -> JudgeResult<ClassifyResponse> {
        Err(JudgeError::Disabled)
    }
    async fn compare(&self, _req: &CompareRequest) -> JudgeResult<CompareResponse> {
        Err(JudgeError::Disabled)
    }
    async fn extract(&self, _req: &ExtractRequest) -> JudgeResult<ExtractResponse> {
        Err(JudgeError::Disabled)
    }
    async fn rerank(&self, _req: &RerankRequest) -> JudgeResult<RerankResponse> {
        Err(JudgeError::Disabled)
    }
    async fn find(&self, _req: &FindRequest) -> JudgeResult<FindResponse> {
        Err(JudgeError::Disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_judge_denies_every_operation() {
        let client = DisabledJudge;
        assert_eq!(client.model_id(), "disabled");
        assert!(matches!(
            client
                .classify(&ClassifyRequest {
                    items: vec![],
                    catalog: vec![],
                    purpose: None,
                })
                .await,
            Err(JudgeError::Disabled)
        ));
        assert!(matches!(
            client
                .compare(&CompareRequest {
                    passage_a: "a".into(),
                    passage_b: "b".into(),
                    aspects: vec![],
                })
                .await,
            Err(JudgeError::Disabled)
        ));
        assert!(matches!(
            client
                .extract(&ExtractRequest {
                    document: "d".into(),
                    fields: vec![],
                })
                .await,
            Err(JudgeError::Disabled)
        ));
        assert!(matches!(
            client
                .rerank(&RerankRequest {
                    query: "q".into(),
                    candidates: vec![],
                })
                .await,
            Err(JudgeError::Disabled)
        ));
        assert!(matches!(
            client
                .find(&FindRequest {
                    query: "q".into(),
                    candidates: vec![],
                })
                .await,
            Err(JudgeError::Disabled)
        ));
    }
}
