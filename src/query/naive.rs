//! Naive (vector-only) search — port of `_op.py::naive_query` (1107-1140).

use serde_json::Value;

use crate::core::rag::QueryParam;
use crate::core::text::truncate_list_by_token_size;
use crate::graph::prompts::{FAIL_RESPONSE, NAIVE_RAG_RESPONSE};
use crate::llm::{LlmError, LlmResult, ModelOptions};
use crate::query::QueryStores;

pub async fn naive_query(
    stores: &QueryStores,
    query: &str,
    param: &QueryParam,
) -> LlmResult<String> {
    let chunks_vdb = stores.chunks_vdb.as_ref().ok_or_else(|| {
        LlmError::Transport("naive RAG is disabled (enable_naive_rag = false)".to_string())
    })?;
    let hits = chunks_vdb
        .query(query, param.top_k)
        .await
        .map_err(|e| LlmError::Transport(format!("chunk search: {e}")))?;
    if hits.is_empty() {
        return Ok(FAIL_RESPONSE.to_string());
    }
    let ids: Vec<String> = hits.iter().map(|hit| hit.id.clone()).collect();
    let rows = stores
        .text_chunks
        .get_by_ids(&ids)
        .await
        .map_err(|e| LlmError::Transport(format!("chunk read: {e}")))?;
    let chunks: Vec<Value> = rows.into_iter().flatten().collect();
    let truncated = truncate_list_by_token_size(
        &chunks,
        |chunk| chunk["content"].as_str().unwrap_or_default().to_string(),
        param.naive_max_token_for_text_unit,
        &stores.tokenizer,
    );
    let section = truncated
        .iter()
        .map(|chunk| chunk["content"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("--New Chunk--\n");
    if param.only_need_context {
        return Ok(section);
    }
    let system_prompt = crate::core::text::fill_template(
        NAIVE_RAG_RESPONSE,
        &[
            ("content_data", section.as_str()),
            ("response_type", param.response_type.as_str()),
        ],
    )
    .map_err(|e| LlmError::Decode(e.to_string()))?;
    let (response, _) = stores
        .llm
        .complete_cached(query, Some(&system_prompt), &[], &ModelOptions::default())
        .await?;
    Ok(response)
}
