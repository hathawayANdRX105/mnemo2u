//! OpenAI-compatible chat client (`_llm.py::openai_complete_if_cache`, :38-64).
//!
//! R4.T0: the HTTP call now runs through the shared execution
//! [`envelope`](crate::llm::envelope) — retry with exponential backoff,
//! deadline timeout, sanitized error classification, metering. Chat is
//! never gated by the egress gate (R1 status quo); the reference-parity
//! retry values (5 attempts, 4s–10s backoff) come from [`OpenAiConfig`].

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::core::rag::ChatMessage;
use crate::llm::envelope::{
    self, retry_after_header, CallMeta, DefaultMeteringSink, EgressGate, MeteringSink, Op, Policy,
    ProviderError, RawCallError,
};
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
    policy: Policy,
    gate: EgressGate,
    sink: std::sync::Arc<dyn MeteringSink>,
}

impl OpenAiClient {
    pub fn new(config: OpenAiConfig) -> Self {
        Self {
            policy: Policy {
                max_attempts: config.max_attempts,
                initial_backoff: config.backoff_min,
                max_backoff: config.backoff_max,
                timeout: Duration::from_secs(60),
            },
            gate: EgressGate::new(false),
            sink: std::sync::Arc::new(DefaultMeteringSink::new()),
            config,
            // A User-Agent is not optional in practice: `reqwest`'s default
            // sends none, and Cloudflare-fronted gateways answer a
            // UA-less request with `405 Method Not Allowed` (observed
            // against the local wildtoken gateway during the real smoke).
            http: reqwest::Client::builder()
                .user_agent(concat!("mnemo2u/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("default HTTP client"),
        }
    }

    pub fn with_http(config: OpenAiConfig, http: reqwest::Client) -> Self {
        let mut client = Self::new(config);
        client.http = http;
        client
    }

    /// Override the envelope effects (tests inject a scripted sink).
    pub fn with_envelope(
        mut self,
        policy: Policy,
        gate: EgressGate,
        sink: std::sync::Arc<dyn MeteringSink>,
    ) -> Self {
        self.policy = policy;
        self.gate = gate;
        self.sink = sink;
        self
    }
}
/// Sanitized [`ProviderError`] → the existing [`LlmError`] surface.
/// Upstream bodies never reach the caller.
fn provider_to_llm(error: ProviderError) -> LlmError {
    match error {
        ProviderError::Timeout { .. } => LlmError::Transport("request timed out".into()),
        ProviderError::RateLimited { .. } => LlmError::Transport("rate limited".into()),
        ProviderError::Unavailable { .. } => LlmError::Transport("backend unavailable".into()),
        ProviderError::AuthFailed { .. } => LlmError::Status {
            status: 401,
            body: "authentication failed".into(),
        },
        ProviderError::InvalidResponse { .. } => LlmError::Decode("invalid response".into()),
        ProviderError::PolicyDenied => LlmError::Transport("egress policy denied".into()),
        ProviderError::Disabled => LlmError::Transport("disabled client invoked".into()),
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

        let http = self.http.clone();
        let api_key = self.config.api_key.clone();
        let model = self.config.model.clone();
        let result = envelope::execute(
            Op::Chat,
            &self.policy,
            &self.gate,
            self.sink.as_ref(),
            || {
                let http = http.clone();
                let url = url.clone();
                let api_key = api_key.clone();
                let body = body.clone();
                let model = model.clone();
                async move {
                    let response = http
                        .post(&url)
                        .bearer_auth(&api_key)
                        .json(&body)
                        .send()
                        .await
                        .map_err(|_| RawCallError::Transport)?;
                    let status = response.status();
                    let retry_after = retry_after_header(response.headers());
                    let text = response.text().await.unwrap_or_default();
                    if status.is_success() {
                        let value: Value = serde_json::from_str(&text)
                            .map_err(|_| RawCallError::InvalidResponse)?;
                        let content = value["choices"][0]["message"]["content"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let meta = CallMeta {
                            model,
                            input_tokens: value["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                            output_tokens: value["usage"]["completion_tokens"]
                                .as_u64()
                                .unwrap_or(0),
                            candidate_scale: 0,
                            from_cache: false,
                        };
                        Ok((content, meta))
                    } else {
                        Err(RawCallError::Http {
                            status: status.as_u16(),
                            retry_after,
                        })
                    }
                }
            },
        )
        .await;
        match result {
            Ok((content, _meta)) => Ok(content),
            Err(error) => Err(provider_to_llm(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default client must send a User-Agent: Cloudflare-fronted
    /// gateways answer UA-less requests with `405 Method Not Allowed`
    /// (observed against the wildtoken gateway during the real smoke).
    #[tokio::test]
    async fn default_client_sends_a_user_agent() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read line");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
                request.push_str(&line);
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).expect("read body");
            let mut stream = stream;
            let payload = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(response.as_bytes()).expect("write");
            request
        });

        let client = OpenAiClient::new(OpenAiConfig::new(format!("http://{addr}/v1"), "k", "m"));
        let answer = client
            .complete("hi", None, &[], &ModelOptions::default())
            .await
            .expect("complete");
        assert_eq!(answer, "ok");
        let request = server.join().expect("server thread");
        assert!(
            request.to_lowercase().contains("user-agent: mnemo2u/"),
            "request headers: {request}"
        );
    }

    /// The reference parity values survive the envelope migration:
    /// 5 attempts, backoff growing 4s → 8s → capped at 10s.
    #[test]
    fn config_maps_to_reference_parity_policy() {
        let client = OpenAiClient::new(OpenAiConfig::new("http://127.0.0.1:1/v1", "k", "m"));
        assert_eq!(client.policy.max_attempts, 5);
        assert_eq!(client.policy.initial_backoff, Duration::from_secs(4));
        assert_eq!(client.policy.max_backoff, Duration::from_secs(10));
    }
}
