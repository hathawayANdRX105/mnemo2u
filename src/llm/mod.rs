//! LLM + embedding adapters.
//!
//! R1 (nano-graphrag port) needs a text LLM for entity/relation extraction and
//! community reports: an OpenAI-compatible HTTP client behind an [`LlmClient`]
//! trait, with a deterministic mock for tests (no network in CI).
//! Embeddings: `fastembed` (ONNX, local; model id feeds the vector cache key).

pub mod cache;
pub mod mock;
pub mod openai;

use async_trait::async_trait;

use crate::core::rag::ChatMessage;

/// Error surface for model clients.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("http status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("decode error: {0}")]
    Decode(String),
    #[error("mock client exhausted its scripted responses")]
    MockExhausted,
}

pub type LlmResult<T> = Result<T, LlmError>;

/// Per-call model options. The reference passes these as kwargs
/// (`max_tokens` for summaries, `response_format={"type": "json_object"}` for
/// community reports and global map steps). NOTE: like the reference
/// (`openai_complete_if_cache`, `_llm.py:52`), the cache key does **not**
/// include these options — only model + messages.
#[derive(Debug, Clone, Default)]
pub struct ModelOptions {
    pub max_tokens: Option<u32>,
    pub json_object: bool,
}

/// Minimal chat-completion client (reference `best_model_func`/`cheap_model_func`).
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Model identity — part of the cache key (`compute_args_hash`).
    fn model_id(&self) -> String;
    async fn complete(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
    ) -> LlmResult<String>;
}

/// Message assembly shared by the cache key and the HTTP client
/// (`openai_complete_if_cache`, `_llm.py:44-50`): system, then history, then prompt.
pub fn build_messages(
    system_prompt: Option<&str>,
    history: &[ChatMessage],
    prompt: &str,
) -> Vec<ChatMessage> {
    let mut messages = Vec::with_capacity(history.len() + 2);
    if let Some(system) = system_prompt {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system.to_string(),
        });
    }
    messages.extend(history.iter().cloned());
    messages.push(ChatMessage::user(prompt));
    messages
}
