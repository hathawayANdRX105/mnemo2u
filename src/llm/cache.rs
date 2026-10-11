//! LLM response cache — reference `openai_complete_if_cache` with `hashing_kv`
//! (`_llm.py:44-61`) and the `{"return", "model"}` value shape.

use std::sync::Arc;

use serde_json::json;

use crate::core::rag::ChatMessage;
use crate::core::text::compute_args_hash;
use crate::core::traits::KvStore;
use crate::llm::{build_messages, LlmClient, LlmError, LlmResult, ModelOptions};

/// Wraps a client with the key-value cache. Both switches start on — the R1
/// default (`enable_llm_cache=True`, `graphrag.py:135`) and the reference's own
/// defaults (`lightrag.py:1093,1096`) — so a ported path keeps caching unless an
/// operator turns a switch off.
#[derive(Clone)]
pub struct CachedLlm {
    inner: Arc<dyn LlmClient>,
    cache: Arc<dyn KvStore>,
    /// `enable_llm_cache` (`lightrag.py:1093`): the global switch, applied to
    /// every cache use other than extraction.
    enable_llm_cache: bool,
    /// `enable_llm_cache_for_entity_extract` (`lightrag.py:1096`): applied to
    /// the `extract` namespace alone.
    enable_llm_cache_for_extract: bool,
}

/// One keyed completion plus the cache row's key.
///
/// The reference hands that key back to the caller through
/// `cache_keys_collector` (`use_llm_func_with_cache`, `utils.py:5651-5667`) so
/// the owning chunk can reference the row (`update_chunk_cache_list`, :5136).
#[derive(Debug, Clone)]
pub struct TrackedCompletion {
    /// The completion text.
    pub text: String,
    /// Whether the cache served it instead of the model.
    pub from_cache: bool,
    /// The `{mode}:{cache_type}:{hash}` row's key, `None` when the call
    /// bypassed the cache (its switch off) — then no row exists to reference.
    pub cache_key: Option<String>,
}

impl CachedLlm {
    /// Wrap a client. Both cache switches are on: the port's default.
    pub fn new(inner: Arc<dyn LlmClient>, cache: Arc<dyn KvStore>) -> Self {
        Self {
            inner,
            cache,
            enable_llm_cache: true,
            enable_llm_cache_for_extract: true,
        }
    }

    /// `enable_llm_cache=False` (`lightrag.py:1093`): with the global switch
    /// off, keyword / summary / answer calls go straight to the model
    /// (`utils.py:5740-5770`).
    pub fn with_enable_llm_cache(mut self, enabled: bool) -> Self {
        self.enable_llm_cache = enabled;
        self
    }

    /// `enable_llm_cache_for_entity_extract=False` (`lightrag.py:1096`): only
    /// the extraction namespace stops caching.
    pub fn with_enable_llm_cache_for_extract(mut self, enabled: bool) -> Self {
        self.enable_llm_cache_for_extract = enabled;
        self
    }

    /// Which switch a cache call follows, mirroring the split in
    /// `handle_cache` (`utils.py:4464-4468`): `extract` follows the
    /// extract-dedicated switch, every other namespace — and the untyped R1
    /// key space — follows the global one.
    pub fn caches(&self, cache_type: Option<&str>) -> bool {
        match cache_type {
            Some("extract") => self.enable_llm_cache_for_extract,
            _ => self.enable_llm_cache,
        }
    }

    pub fn inner(&self) -> Arc<dyn LlmClient> {
        self.inner.clone()
    }

    /// Returns the completion plus whether it came from the cache.
    ///
    /// R1 key: the bare `compute_args_hash`, matching nano-graphrag's
    /// `hashing_kv` keying. R2's ported paths use [`Self::complete_cached_keyed`]
    /// instead, so the two references never share a key space.
    pub async fn complete_cached(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
    ) -> LlmResult<(String, bool)> {
        let (text, hit, _) = self
            .complete_with_key(prompt, system_prompt, history, options, None)
            .await?;
        Ok((text, hit))
    }

    /// LightRAG `generate_cache_key` (`utils.py:1090`): the flattened
    /// `{mode}:{cache_type}:{hash}` key. `cache_type` partitions the namespaces
    /// the reference keeps apart (`extract`, `keywords`, `query`, `summary`),
    /// so a summarisation call can never be served by an extraction response.
    pub async fn complete_cached_keyed(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
        cache_type: &str,
    ) -> LlmResult<(String, bool)> {
        let (text, hit, _) = self
            .complete_with_key(prompt, system_prompt, history, options, Some(cache_type))
            .await?;
        Ok((text, hit))
    }

    /// [`Self::complete_cached_keyed`] plus the cache row's key, for callers
    /// that must reference the row on the chunk that triggered it
    /// (`cache_keys_collector`, `utils.py:5651-5667`).
    pub async fn complete_cached_keyed_tracked(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
        cache_type: &str,
    ) -> LlmResult<TrackedCompletion> {
        let (text, from_cache, cache_key) = self
            .complete_with_key(prompt, system_prompt, history, options, Some(cache_type))
            .await?;
        Ok(TrackedCompletion {
            text,
            from_cache,
            cache_key,
        })
    }

    /// A direct model call that ignores the cache in both directions. Keyword
    /// parsing retries use it: replaying the response that just failed to parse
    /// cannot parse any better the second time.
    pub async fn complete_uncached(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
    ) -> LlmResult<String> {
        self.inner
            .complete(prompt, system_prompt, history, options)
            .await
    }

    /// Overwrite one cache row (`save_to_cache`'s update path,
    /// `utils.py:4499-4520`). The keyword step uses it so a parseable retry
    /// replaces the unparseable response the first attempt cached.
    pub async fn cache_put_keyed(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        cache_type: &str,
        text: &str,
    ) -> LlmResult<()> {
        if !self.caches(Some(cache_type)) {
            return Ok(());
        }
        let key = self.key_for(prompt, system_prompt, history, Some(cache_type));
        self.cache
            .upsert(vec![(
                key,
                json!({"return": text, "model": self.inner.model_id()}),
            )])
            .await
            .map_err(|e| LlmError::Transport(format!("cache write: {e}")))?;
        self.cache
            .index_done()
            .await
            .map_err(|e| LlmError::Transport(format!("cache flush: {e}")))?;
        Ok(())
    }

    /// `generate_cache_key` (`utils.py:1090`): the flattened
    /// `{mode}:{cache_type}:{hash}` key, or the bare R1 hash when the call is
    /// untyped.
    fn key_for(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        cache_type: Option<&str>,
    ) -> String {
        let model = self.inner.model_id();
        let messages = build_messages(system_prompt, history, prompt);
        let args_hash = compute_args_hash(&model, &messages);
        match cache_type {
            Some(cache_type) => format!("default:{cache_type}:{args_hash}"),
            None => args_hash,
        }
    }

    /// Text, whether the cache served it, and the key of the row it read or
    /// wrote (`None` when the switch for that namespace is off, in which case
    /// nothing was read and nothing was written).
    async fn complete_with_key(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
        cache_type: Option<&str>,
    ) -> LlmResult<(String, bool, Option<String>)> {
        let key = self.key_for(prompt, system_prompt, history, cache_type);
        let enabled = self.caches(cache_type);

        if enabled {
            if let Some(hit) = self
                .cache
                .get_by_id(&key)
                .await
                .map_err(|e| LlmError::Transport(format!("cache read: {e}")))?
            {
                if let Some(text) = hit["return"].as_str() {
                    return Ok((text.to_string(), true, Some(key)));
                }
            }
        }

        let text = self
            .inner
            .complete(prompt, system_prompt, history, options)
            .await?;
        if enabled {
            let model = self.inner.model_id();
            self.cache
                .upsert(vec![(
                    key.clone(),
                    json!({"return": text.clone(), "model": model}),
                )])
                .await
                .map_err(|e| LlmError::Transport(format!("cache write: {e}")))?;
            self.cache
                .index_done()
                .await
                .map_err(|e| LlmError::Transport(format!("cache flush: {e}")))?;
        }
        Ok((text, false, enabled.then_some(key)))
    }
}
