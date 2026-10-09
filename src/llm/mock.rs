//! Deterministic mocks: scripted LLM + hash-based embedder (no network).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use md5::{Digest, Md5};

use crate::core::rag::ChatMessage;
use crate::core::traits::{Embedder, Result as StoreResult};
use crate::llm::{LlmClient, LlmError, LlmResult};

/// Scripted LLM: answers in script order and counts calls.
///
/// Exhausting the script is an error, so unexpected calls (e.g. a cache miss
/// that should have been a hit) fail loudly instead of silently succeeding.
pub struct MockLlm {
    model: String,
    script: Mutex<VecDeque<String>>,
    calls: AtomicUsize,
    sent_prompts: Mutex<Vec<String>>,
}

impl MockLlm {
    pub fn new(model: impl Into<String>, script: Vec<String>) -> Self {
        Self {
            model: model.into(),
            script: Mutex::new(script.into()),
            calls: AtomicUsize::new(0),
            sent_prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Every prompt this mock has received, in call order.
    pub fn sent_prompts(&self) -> Vec<String> {
        self.sent_prompts.lock().expect("mock lock").clone()
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    fn model_id(&self) -> String {
        self.model.clone()
    }

    async fn complete(
        &self,
        prompt: &str,
        _system_prompt: Option<&str>,
        _history: &[ChatMessage],
        _options: &crate::llm::ModelOptions,
    ) -> LlmResult<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.sent_prompts
            .lock()
            .expect("mock lock")
            .push(prompt.to_string());
        self.script
            .lock()
            .expect("mock lock")
            .pop_front()
            .ok_or(LlmError::MockExhausted)
    }
}

/// Content-routed mock: picks the first rule whose matcher is contained in the
/// prompt. Robust against call-order changes when many pipeline stages share
/// one client (extraction, summaries, reports, answers).
pub struct RoutedLlm {
    model: String,
    rules: Vec<(String, String)>,
    calls: AtomicUsize,
    unmatched: Mutex<Vec<String>>,
}

impl RoutedLlm {
    pub fn new(model: impl Into<String>, rules: Vec<(String, String)>) -> Self {
        Self {
            model: model.into(),
            rules,
            calls: AtomicUsize::new(0),
            unmatched: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Prompts that matched no rule (all of them are also errors).
    pub fn unmatched_prompts(&self) -> Vec<String> {
        self.unmatched.lock().expect("mock lock").clone()
    }
}

#[async_trait]
impl LlmClient for RoutedLlm {
    fn model_id(&self) -> String {
        self.model.clone()
    }

    async fn complete(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        _history: &[ChatMessage],
        _options: &crate::llm::ModelOptions,
    ) -> LlmResult<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let haystack = format!("{}\n{}", prompt, system_prompt.unwrap_or_default());
        for (matcher, response) in &self.rules {
            if haystack.contains(matcher) {
                return Ok(response.clone());
            }
        }
        self.unmatched
            .lock()
            .expect("mock lock")
            .push(prompt.to_string());
        Err(LlmError::MockExhausted)
    }
}

/// Deterministic embedder for offline tests: hashing bag-of-words.
///
/// Tokens are hashed into buckets and the vector is L2-normalised, so cosine
/// similarity approximates word overlap — enough for retrieval-path tests
/// without a model download. Semantic quality is explicitly out of scope.
pub struct MockEmbedder {
    dim: usize,
    max_token_size: usize,
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self {
            dim: 64,
            max_token_size: 8192,
        }
    }
}

impl MockEmbedder {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Embedder for MockEmbedder {
    fn model_id(&self) -> String {
        "mock-embed-64".to_string()
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn max_token_size(&self) -> usize {
        self.max_token_size
    }

    async fn embed(&self, texts: &[String]) -> StoreResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|text| self.embed_one(text)).collect())
    }
}

impl MockEmbedder {
    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0f32; self.dim];
        for token in text.split(|c: char| !c.is_alphanumeric()) {
            let token = token.to_lowercase();
            if token.is_empty() {
                continue;
            }
            let mut hasher = Md5::new();
            hasher.update(token.as_bytes());
            let digest = hasher.finalize();
            let mut bucket = 0usize;
            for byte in digest.iter().take(8) {
                bucket = (bucket << 8) | *byte as usize;
            }
            vector[bucket % self.dim] += 1.0;
        }
        let norm: f32 = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in vector.iter_mut() {
                *value /= norm;
            }
        }
        vector
    }
}
