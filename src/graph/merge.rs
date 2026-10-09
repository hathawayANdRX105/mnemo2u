//! Merge — port of `_op.py::_merge_nodes_then_upsert` (182-227),
//! `_merge_edges_then_upsert` (230-279) and `_handle_entity_relation_summary`
//! (111-135).
//!
//! Deviation (documented in the stage doc): the reference joins `source_id`
//! from a Python `set`, whose iteration order is implementation-defined; we
//! keep first-encounter order so results are reproducible.

use serde_json::{json, Value};

use crate::core::rag::{EntityRecord, RelationRecord};
use crate::core::text::{
    compute_mdhash_id, py_repr_str_list, py_repr_tuple2, split_string_by_multi_markers, Tokenizer,
    GRAPH_FIELD_SEP,
};
use crate::core::traits::{GraphStore, VectorRow};
use crate::graph::prompts::SUMMARIZE_ENTITY_DESCRIPTIONS;
use crate::llm::cache::CachedLlm;
use crate::llm::{LlmError, LlmResult, ModelOptions};

/// `entity_summary_to_max_tokens` default (`graphrag.py:84`).
pub const DEFAULT_ENTITY_SUMMARY_TO_MAX_TOKENS: usize = 500;
/// `cheap_model_max_token_size` default (`graphrag.py:122`).
pub const DEFAULT_CHEAP_MODEL_MAX_TOKEN_SIZE: usize = 32_768;

#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub entity_summary_to_max_tokens: usize,
    pub cheap_model_max_token_size: usize,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            entity_summary_to_max_tokens: DEFAULT_ENTITY_SUMMARY_TO_MAX_TOKENS,
            cheap_model_max_token_size: DEFAULT_CHEAP_MODEL_MAX_TOKEN_SIZE,
        }
    }
}

fn join_with_sep(values: impl IntoIterator<Item = String>) -> String {
    values.into_iter().collect::<Vec<_>>().join(GRAPH_FIELD_SEP)
}

/// First-encounter-ordered dedup (stand-in for Python `set`).
fn dedup_preserving_order(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for value in values {
        if !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

/// `_handle_entity_relation_summary` (`_op.py:111-135`).
pub async fn handle_entity_relation_summary(
    entity_or_relation_name: &str,
    description: &str,
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &MergeOptions,
) -> LlmResult<String> {
    let tokens = tokenizer.encode(description);
    if tokens.len() < options.entity_summary_to_max_tokens {
        return Ok(description.to_string());
    }
    let use_description = tokenizer
        .decode(&tokens[..tokens.len().min(options.cheap_model_max_token_size)])
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    let description_list: Vec<String> = use_description
        .split(GRAPH_FIELD_SEP)
        .map(|s| s.to_string())
        .collect();
    let description_list_repr = py_repr_str_list(&description_list);
    let prompt = crate::core::text::fill_template(
        SUMMARIZE_ENTITY_DESCRIPTIONS,
        &[
            ("entity_name", entity_or_relation_name),
            ("description_list", description_list_repr.as_str()),
        ],
    )
    .map_err(|e| LlmError::Decode(format!("summary template: {e}")))?;
    let (summary, _) = llm
        .complete_cached(
            &prompt,
            None,
            &[],
            &ModelOptions {
                max_tokens: Some(options.entity_summary_to_max_tokens as u32),
                json_object: false,
            },
        )
        .await?;
    Ok(summary)
}

/// `_merge_nodes_then_upsert` — returns the node data row (with `entity_name`).
pub async fn merge_nodes_then_upsert(
    entity_name: &str,
    nodes_data: &[EntityRecord],
    graph: &dyn GraphStore,
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &MergeOptions,
) -> LlmResult<Value> {
    let already_node = graph
        .get_node(entity_name)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let mut already_entity_types: Vec<String> = Vec::new();
    let mut already_source_ids: Vec<String> = Vec::new();
    let mut already_descriptions: Vec<String> = Vec::new();
    if let Some(node) = &already_node {
        already_entity_types.push(node["entity_type"].as_str().unwrap_or_default().to_string());
        already_source_ids.extend(split_string_by_multi_markers(
            node["source_id"].as_str().unwrap_or_default(),
            &[GRAPH_FIELD_SEP],
        ));
        already_descriptions.push(node["description"].as_str().unwrap_or_default().to_string());
    }

    // Counter.most_common semantics: highest count wins, ties keep the first
    // appearance (the reference sorts stably by count).
    let mut candidates: Vec<String> = Vec::new();
    for value in nodes_data
        .iter()
        .map(|data| data.entity_type.clone())
        .chain(already_entity_types.iter().cloned())
    {
        if !candidates.contains(&value) {
            candidates.push(value);
        }
    }
    let mut entity_type = String::new();
    let mut best_count = 0usize;
    for candidate in &candidates {
        let count = nodes_data
            .iter()
            .filter(|data| data.entity_type == *candidate)
            .count()
            + already_entity_types
                .iter()
                .filter(|value| *value == candidate)
                .count();
        if count > best_count {
            best_count = count;
            entity_type = candidate.clone();
        }
    }

    let description_sorted: std::collections::BTreeSet<String> = nodes_data
        .iter()
        .map(|data| data.description.clone())
        .chain(already_descriptions.iter().cloned())
        .collect();
    let description = join_with_sep(description_sorted);

    let source_id = join_with_sep(dedup_preserving_order(
        nodes_data
            .iter()
            .map(|data| data.source_id.clone())
            .chain(already_source_ids.iter().cloned()),
    ));

    let description =
        handle_entity_relation_summary(entity_name, &description, llm, tokenizer, options).await?;

    let mut node_data = json!({
        "entity_type": entity_type,
        "description": description,
        "source_id": source_id,
    });
    graph
        .upsert_node(entity_name, node_data.clone())
        .await
        .map_err(|e| LlmError::Transport(format!("graph write: {e}")))?;
    node_data["entity_name"] = json!(entity_name);
    Ok(node_data)
}

/// `_merge_edges_then_upsert` — returns nothing; the reference writes through.
pub async fn merge_edges_then_upsert(
    src_id: &str,
    tgt_id: &str,
    edges_data: &[RelationRecord],
    graph: &dyn GraphStore,
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &MergeOptions,
) -> LlmResult<()> {
    let mut already_weights: Vec<f64> = Vec::new();
    let mut already_source_ids: Vec<String> = Vec::new();
    let mut already_descriptions: Vec<String> = Vec::new();
    let mut already_order: Vec<i64> = Vec::new();
    if graph
        .has_edge(src_id, tgt_id)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?
    {
        if let Some(edge) = graph
            .get_edge(src_id, tgt_id)
            .await
            .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?
        {
            already_weights.push(edge["weight"].as_f64().unwrap_or(1.0));
            already_source_ids.extend(split_string_by_multi_markers(
                edge["source_id"].as_str().unwrap_or_default(),
                &[GRAPH_FIELD_SEP],
            ));
            already_descriptions.push(edge["description"].as_str().unwrap_or_default().to_string());
            already_order.push(edge.get("order").and_then(Value::as_i64).unwrap_or(1));
        }
    }

    let order = edges_data
        .iter()
        .map(|data| data.order)
        .chain(already_order.iter().copied())
        .min()
        .unwrap_or(1);
    let weight = edges_data.iter().map(|data| data.weight).sum::<f64>()
        + already_weights.iter().sum::<f64>();

    let description_sorted: std::collections::BTreeSet<String> = edges_data
        .iter()
        .map(|data| data.description.clone())
        .chain(already_descriptions.iter().cloned())
        .collect();
    let description = join_with_sep(description_sorted);
    let source_id = join_with_sep(dedup_preserving_order(
        edges_data
            .iter()
            .map(|data| data.source_id.clone())
            .chain(already_source_ids.iter().cloned()),
    ));

    for endpoint in [src_id, tgt_id] {
        let exists = graph
            .has_node(endpoint)
            .await
            .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
        if !exists {
            graph
                .upsert_node(
                    endpoint,
                    json!({
                        "source_id": source_id,
                        "description": description,
                        "entity_type": "\"UNKNOWN\"",
                    }),
                )
                .await
                .map_err(|e| LlmError::Transport(format!("graph write: {e}")))?;
        }
    }

    let relation_name = py_repr_tuple2(src_id, tgt_id);
    let description =
        handle_entity_relation_summary(&relation_name, &description, llm, tokenizer, options)
            .await?;

    graph
        .upsert_edge(
            src_id,
            tgt_id,
            json!({
                "weight": weight,
                "description": description,
                "source_id": source_id,
                "order": order,
            }),
        )
        .await
        .map_err(|e| LlmError::Transport(format!("graph write: {e}")))?;
    Ok(())
}

/// Vector rows for merged nodes — reference behaviour inside `extract_entities`
/// (`_op.py:403-413`): id hashes the name, content is `name + description`.
pub fn entity_vector_rows(merged_nodes: &[Value]) -> Vec<VectorRow> {
    merged_nodes
        .iter()
        .map(|node| {
            let name = node["entity_name"].as_str().unwrap_or_default();
            VectorRow {
                id: compute_mdhash_id(name, "ent-"),
                content: format!("{name}{}", node["description"].as_str().unwrap_or_default()),
                meta: json!({ "entity_name": name }),
            }
        })
        .collect()
}
