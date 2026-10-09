//! LLM response cache — reference `openai_complete_if_cache` with `hashing_kv`
//! (`_llm.py:44-61`) and the `{"return", "model"}` value shape.

use std::sync::Arc;

use serde_json::json;

use crate::core::rag::ChatMessage;
use crate::core::text::compute_args_hash;
use crate::core::traits::KvStore;
use crate::llm::{build_messages, LlmClient, LlmError, LlmResult, ModelOptions};

/// Wraps a client with the key-value cache (`enable_llm_cache=True` default,
/// `graphrag.py:135`).
#[derive(Clone)]
pub struct CachedLlm {
    inner: Arc<dyn LlmClient>,
    cache: Arc<dyn KvStore>,
}

impl CachedLlm {
    pub fn new(inner: Arc<dyn LlmClient>, cache: Arc<dyn KvStore>) -> Self {
        Self { inner, cache }
    }

    pub fn inner(&self) -> Arc<dyn LlmClient> {
        self.inner.clone()
    }

    /// Returns the completion plus whether it came from the cache.
    pub async fn complete_cached(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
    ) -> LlmResult<(String, bool)> {
        let model = self.inner.model_id();
        let messages = build_messages(system_prompt, history, prompt);
        let args_hash = compute_args_hash(&model, &messages);

        if let Some(hit) = self
            .cache
            .get_by_id(&args_hash)
            .await
            .map_err(|e| LlmError::Transport(format!("cache read: {e}")))?
        {
            if let Some(text) = hit["return"].as_str() {
                return Ok((text.to_string(), true));
            }
        }

        let text = self
            .inner
            .complete(prompt, system_prompt, history, options)
            .await?;
        self.cache
            .upsert(vec![(args_hash, json!({"return": text, "model": model}))])
            .await
            .map_err(|e| LlmError::Transport(format!("cache write: {e}")))?;
        self.cache
            .index_done()
            .await
            .map_err(|e| LlmError::Transport(format!("cache flush: {e}")))?;
        Ok((text, false))
    }
}
