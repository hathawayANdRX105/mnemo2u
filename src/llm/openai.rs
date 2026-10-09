//! OpenAI-compatible chat client (`_llm.py::openai_complete_if_cache`, :38-64).
//!
//! Retry policy mirrors the reference decorator: 5 attempts, exponential
//! backoff between 4s and 10s, retrying on rate limits and connection errors.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::core::rag::ChatMessage;
use crate::llm::{build_messages, LlmClient, LlmError, LlmResult, ModelOptions};

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL including the API version segment, e.g. `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_attempts: u32,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl OpenAiConfig {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            max_attempts: 5,
            backoff_min: Duration::from_secs(4),
            backoff_max: Duration::from_secs(10),
        }
    }
}

pub struct OpenAiClient {
    config: OpenAiConfig,
    http: reqwest::Client,
}

impl OpenAiClient {
    pub fn new(config: OpenAiConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    pub fn with_http(config: OpenAiConfig, http: reqwest::Client) -> Self {
        Self { config, http }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let exponential = self
            .config
            .backoff_min
            .saturating_mul(1u32 << (attempt - 1).min(16));
        exponential
            .min(self.config.backoff_max)
            .max(self.config.backoff_min)
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    fn model_id(&self) -> String {
        self.config.model.clone()
    }

    async fn complete(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        history: &[ChatMessage],
        options: &ModelOptions,
    ) -> LlmResult<String> {
        let messages = build_messages(system_prompt, history, prompt);
        let mut body = json!({
            "model": self.config.model,
            "messages": messages
                .iter()
                .map(|m| json!({"role": m.role, "content": m.content}))
                .collect::<Vec<_>>(),
        });
        if let Some(max_tokens) = options.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if options.json_object {
            body["response_format"] = json!({"type": "json_object"});
        }
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let outcome = self
                .http
                .post(&url)
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await;
            match outcome {
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    if status.is_success() {
                        let value: Value = serde_json::from_str(&text).map_err(|e| {
                            LlmError::Decode(format!(
                                "{e}: {}",
                                text.chars().take(200).collect::<String>()
                            ))
                        })?;
                        return Ok(value["choices"][0]["message"]["content"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string());
                    }
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if retryable && attempt < self.config.max_attempts {
                        tokio::time::sleep(self.backoff(attempt)).await;
                        continue;
                    }
                    return Err(LlmError::Status {
                        status: status.as_u16(),
                        body: text,
                    });
                }
                Err(err) => {
                    if attempt < self.config.max_attempts {
                        tokio::time::sleep(self.backoff(attempt)).await;
                        continue;
                    }
                    return Err(LlmError::Transport(err.to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Backoff mirrors tenacity's `wait_exponential(multiplier=1, min=4, max=10)`:
    /// 4s, 8s, 10s, 10s …
    #[test]
    fn backoff_sequence_matches_reference() {
        let client = OpenAiClient::new(OpenAiConfig::new("http://127.0.0.1:1/v1", "k", "m"));
        let delays: Vec<u64> = (1..=4).map(|i| client.backoff(i).as_secs()).collect();
        assert_eq!(delays, vec![4, 8, 10, 10]);
    }
}
