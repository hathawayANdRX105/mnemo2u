//! Judgment response cache (R4.T6).
//!
//! Key: `judge:{op}:{hash}` — the hash covers
//! `model + tool + request` (the doc's
//! `hash(model, tool, state, questions)`), so the
//! same judgment is never re-rolled: a hit returns
//! the cached response DTO without a wire call.
//!
//! The row shape mirrors the LLM cache
//! (`{"return", "model"}`), and the value is the
//! serialized response DTO, so a hit round-trips
//! through the same type the backend would have
//! returned.

use std::sync::Arc;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::core::text::md5_hex;
use crate::core::traits::KvStore;
use crate::llm::envelope::{Op, ProviderError};
use crate::llm::judge::{
    ClassifyRequest, ClassifyResponse, CompareRequest, CompareResponse, ExtractRequest,
    ExtractResponse, FindRequest, FindResponse, JudgeClient, JudgeError, JudgeResult,
    RerankRequest, RerankResponse,
};

/// Wraps a [`JudgeClient`] with the key-value cache. Every
/// operation reads the cache before the backend and writes
/// the backend's answer back; a cache failure fails the
/// call, mirroring the LLM cache's error semantics.
#[derive(Clone)]
pub struct CachedJudge {
    inner: Arc<dyn JudgeClient>,
    cache: Arc<dyn KvStore>,
}

impl CachedJudge {
    pub fn new(inner: Arc<dyn JudgeClient>, cache: Arc<dyn KvStore>) -> Self {
        Self { inner, cache }
    }

    pub fn inner(&self) -> Arc<dyn JudgeClient> {
        self.inner.clone()
    }

    /// `judge:{op}:{md5(model, serialized request)}` — the
    /// model id rides the key so a model change is a cache
    /// miss, not a stale answer.
    fn key_for<T: serde::Serialize>(&self, op: &str, request: &T) -> JudgeResult<String> {
        let model = self.inner.model_id();
        let request = serde_json::to_string(request).map_err(|_| JudgeError::InvalidResponse)?;
        let hash = md5_hex(&format!("({model:?}, {request})"));
        Ok(format!("judge:{op}:{hash}"))
    }

    async fn cached<R: DeserializeOwned>(&self, key: &str) -> JudgeResult<Option<R>> {
        let hit = self.cache.get_by_id(key).await.map_err(cache_error)?;
        let Some(hit) = hit else {
            return Ok(None);
        };
        let Some(value) = hit.get("return") else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| JudgeError::InvalidResponse)
    }

    async fn store<R: serde::Serialize>(&self, key: &str, response: &R) -> JudgeResult<()> {
        self.cache
            .upsert(vec![(
                key.to_string(),
                json!({
                    "return": response,
                    "model": self.inner.model_id(),
                }),
            )])
            .await
            .map_err(cache_error)?;
        self.cache.index_done().await.map_err(cache_error)?;
        Ok(())
    }

    /// One operation through the cache: read, else call the
    /// backend and write the answer back.
    async fn through<Req: serde::Serialize, Resp: DeserializeOwned + serde::Serialize>(
        &self,
        op: &str,
        request: &Req,
        call: impl std::future::Future<Output = JudgeResult<Resp>>,
    ) -> JudgeResult<Resp> {
        let key = self.key_for(op, request)?;
        if let Some(hit) = self.cached::<Resp>(&key).await? {
            return Ok(hit);
        }
        let response = call.await?;
        self.store(&key, &response).await?;
        Ok(response)
    }
}

/// A cache failure is a backend failure of the judge
/// capability — the call cannot be served or recorded.
fn cache_error(error: impl std::fmt::Display) -> JudgeError {
    tracing::warn!(%error, "judge cache failed");
    JudgeError::Backend(ProviderError::Unavailable { op: Op::Judge })
}

#[async_trait]
impl JudgeClient for CachedJudge {
    fn model_id(&self) -> String {
        self.inner.model_id()
    }

    async fn classify(&self, req: &ClassifyRequest) -> JudgeResult<ClassifyResponse> {
        self.through("classify", req, self.inner.classify(req))
            .await
    }

    async fn compare(&self, req: &CompareRequest) -> JudgeResult<CompareResponse> {
        self.through("compare", req, self.inner.compare(req)).await
    }

    async fn extract(&self, req: &ExtractRequest) -> JudgeResult<ExtractResponse> {
        self.through("extract", req, self.inner.extract(req)).await
    }

    async fn rerank(&self, req: &RerankRequest) -> JudgeResult<RerankResponse> {
        self.through("rerank", req, self.inner.rerank(req)).await
    }

    async fn find(&self, req: &FindRequest) -> JudgeResult<FindResponse> {
        self.through("find", req, self.inner.find(req)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::judge::{ExistsVerdict, RerankCandidate, Verdict};
    use crate::llm::judge_mock::JudgeMock;
    use crate::store::memory::MemoryKv;

    fn classify_request() -> ClassifyRequest {
        ClassifyRequest {
            items: vec![crate::llm::judge::ClassifyItem {
                id: "c0".into(),
                text: "ACME acquired Beta Labs".into(),
            }],
            catalog: vec![crate::llm::judge::ClassifyClass {
                id: "semantic".into(),
                description: "a durable statement".into(),
            }],
            purpose: Some("write typing".into()),
        }
    }

    #[tokio::test]
    async fn a_hit_does_not_reroll_the_judgment() {
        let inner = Arc::new(JudgeMock::new(
            "judge-mock",
            vec![JudgeMock::classify_all(
                &["c0"],
                "semantic",
                Verdict::Yes,
                0.9,
            )],
        ));
        let cached = Arc::new(CachedJudge::new(
            Arc::clone(&inner) as Arc<dyn JudgeClient>,
            Arc::new(MemoryKv::new()),
        ));
        let client: Arc<dyn JudgeClient> = cached;

        let first = client
            .classify(&classify_request())
            .await
            .expect("first call");
        assert_eq!(first.verdicts.len(), 1);
        // The mock's script holds one reply: a second call that
        // reaches it would panic (script exhausted).
        let second = client
            .classify(&classify_request())
            .await
            .expect("cache hit");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn a_different_model_is_a_cache_miss() {
        let inner = Arc::new(JudgeMock::new(
            "judge-mock",
            vec![
                JudgeMock::classify_all(&["c0"], "semantic", Verdict::Yes, 0.9),
                JudgeMock::classify_all(&["c0"], "episodic", Verdict::Review, 0.5),
            ],
        ));
        let cache = Arc::new(MemoryKv::new());
        let first = CachedJudge::new(
            Arc::clone(&inner) as Arc<dyn JudgeClient>,
            Arc::clone(&cache) as Arc<dyn KvStore>,
        );
        let second = CachedJudge::new(
            Arc::new(JudgeMock::new(
                "judge-mock-2",
                vec![JudgeMock::classify_all(
                    &["c0"],
                    "episodic",
                    Verdict::Review,
                    0.5,
                )],
            )) as Arc<dyn JudgeClient>,
            cache,
        );

        let a = first.classify(&classify_request()).await.expect("first");
        assert_eq!(a.verdicts[0].class_id, "semantic");
        let b = second.classify(&classify_request()).await.expect("second");
        assert_eq!(b.verdicts[0].class_id, "episodic");
    }

    #[tokio::test]
    async fn find_round_trips_through_the_cache() {
        let inner = Arc::new(JudgeMock::new(
            "judge-mock",
            vec![
                JudgeMock::find(Some(("f-1", 0.8)), ExistsVerdict::Answered),
                JudgeMock::find(Some(("f-1", 0.8)), ExistsVerdict::Answered),
            ],
        ));
        let client: Arc<dyn JudgeClient> =
            Arc::new(CachedJudge::new(inner, Arc::new(MemoryKv::new())));
        let request = FindRequest {
            query: "who acquired ACME".into(),
            candidates: vec![RerankCandidate {
                id: "f-1".into(),
                text: "ACME bought Beta Labs".into(),
            }],
        };

        let first = client.find(&request).await.expect("first call");
        let second = client.find(&request).await.expect("cache hit");
        assert_eq!(first, second);
        assert_eq!(
            first.best.as_ref().map(|hit| hit.id.clone()),
            Some("f-1".to_string())
        );
        assert_eq!(first.exists_verdict, ExistsVerdict::Answered);
        // The cache served the second call: the mock's one
        // scripted reply was consumed exactly once.
        assert_eq!(first.best, second.best);
    }
}
