//! Dual keyword extraction — LightRAG's `extract_keywords_only`
//! (`operate.py:5112`) and its response parser.
//!
//! The model returns one JSON object with `high_level_keywords` (themes) and
//! `low_level_keywords` (concrete entities); the two lists then drive the
//! relation arm and the entity arm respectively (`kg_query`, operate.py:4693).
//! The LLM call is cached under `cache_type="keywords"`.

use serde_json::Value;

use crate::core::rag::ChatMessage;
use crate::graph::prompts::{KEYWORDS_EXTRACTION, KEYWORDS_EXTRACTION_EXAMPLES};
use crate::llm::cache::CachedLlm;
use crate::llm::{LlmError, LlmResult, ModelOptions};

/// What the keyword step produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryKeywords {
    /// Themes → the `relationships_vdb` arm.
    pub high: Vec<String>,
    /// Concrete entities → the `entities_vdb` arm.
    pub low: Vec<String>,
}

impl QueryKeywords {
    pub fn high_joined(&self) -> String {
        self.high.join(", ")
    }

    pub fn low_joined(&self) -> String {
        self.low.join(", ")
    }
}

/// Extract both keyword lists for one query (`extract_keywords_only`).
pub async fn extract_keywords_only(llm: &CachedLlm, query: &str) -> LlmResult<QueryKeywords> {
    let prompt = crate::core::text::fill_template(
        KEYWORDS_EXTRACTION,
        &[("examples", KEYWORDS_EXTRACTION_EXAMPLES), ("query", query)],
    )
    .map_err(|e| LlmError::Decode(format!("keywords template: {e}")))?;
    let (response, _) = llm
        .complete_cached_keyed(
            &prompt,
            None,
            &[],
            &ModelOptions {
                json_object: true,
                ..ModelOptions::default()
            },
            "keywords",
        )
        .await?;
    Ok(parse_keywords_response(&response).unwrap_or_default())
}

/// Parse the keyword JSON object. The reference is strict: one object, exactly
/// two keys, both string arrays. Anything else (fenced block, prose, wrong
/// shape) is a parse failure the caller treats as "no keywords".
///
/// The reference locates the JSON payload with a brace scan and then
/// `json.loads`; we mirror the "first balanced object" search, then apply the
/// shape rules.
pub fn parse_keywords_response(text: &str) -> Option<QueryKeywords> {
    let payload = locate_json_object(text)?;
    let value: Value = serde_json::from_str(&payload).ok()?;
    let object = value.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let high = string_array(object.get("high_level_keywords"))?;
    let low = string_array(object.get("low_level_keywords"))?;
    Some(QueryKeywords { high, low })
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let items = value?.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

/// First brace-balanced object in the text, so a fenced or chatter-wrapped
/// response still parses. Returns `None` when the text has no object.
fn locate_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + index + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Cache-key inputs shared with the answer cache (kept next to the keyword
/// parser so the two surfaces cannot drift): the reference hashes mode, query,
/// response type, the token budgets and both keyword lists
/// (`operate.py:4858-4891`).
pub fn answer_cache_inputs(param: &crate::core::rag::QueryParam) -> Vec<String> {
    vec![
        param.mode.as_str().to_string(),
        param.top_k.to_string(),
        param.chunk_top_k.to_string(),
        param.max_entity_tokens.to_string(),
        param.max_relation_tokens.to_string(),
        param.max_total_tokens.to_string(),
        param.enable_rerank.to_string(),
    ]
}

impl crate::core::rag::QueryMode {
    /// The reference's literal mode strings (`base.py:93`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Global => "global",
            Self::Naive => "naive",
            Self::Hybrid => "hybrid",
            Self::Mix => "mix",
        }
    }
}

/// Message helper for the answer call, kept here because the prompt assembly
/// lives with the keyword step (`kg_query` builds system + user).
pub fn answer_messages(
    system_prompt: &str,
    query: &str,
    history: &[ChatMessage],
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system_prompt.to_string(),
    }];
    messages.extend(history.iter().cloned());
    messages.push(ChatMessage::user(query.to_string()));
    messages
}
