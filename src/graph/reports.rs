//! Community reports — port of `_op.py::_pack_single_community_by_sub_communities`
//! (417-461), `_pack_single_community_describe` (464-600),
//! `_community_report_json_to_str` (603-622) and `generate_community_report`
//! (625-697).
//!
//! Reports are generated level by level, coarsest first, so each level can
//! reuse the reports of its sub-communities as context.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};
use tokio::task::JoinSet;

use crate::core::rag::CommunitySchema;
use crate::core::text::{
    convert_response_to_json, fill_template, list_of_list_to_csv, truncate_list_by_token_size,
    Tokenizer,
};
use crate::core::traits::GraphStore;
use crate::graph::prompts::COMMUNITY_REPORT;
use crate::llm::cache::CachedLlm;
use crate::llm::{LlmError, LlmResult, ModelOptions};

/// `best_model_max_token_size` default (`graphrag.py:118`).
pub const DEFAULT_BEST_MODEL_MAX_TOKEN_SIZE: usize = 32_768;
/// Community descriptions switch to sub-communities above this size
/// (`_pack_single_community_describe`, `_op.py:496`).
pub const TRUNCATION_NODE_LIMIT: usize = 100;
/// Headroom kept for the chat template (`_op.py:655`).
pub const REPORT_PROMPT_HEADROOM: usize = 200;

#[derive(Debug, Clone)]
pub struct ReportOptions {
    pub best_model_max_token_size: usize,
    /// `special_community_report_llm_kwargs` → `response_format={"type":"json_object"}`
    /// (`graphrag.py:101`).
    pub json_object: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            best_model_max_token_size: DEFAULT_BEST_MODEL_MAX_TOKEN_SIZE,
            json_object: true,
        }
    }
}

/// `_community_report_json_to_str` (`_op.py:603-622`).
pub fn community_report_json_to_str(parsed: &Value) -> String {
    let title = parsed
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Report");
    let summary = parsed.get("summary").and_then(Value::as_str).unwrap_or("");
    let findings = parsed
        .get("findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let sections = findings
        .iter()
        .map(|finding| {
            let (summary, explanation) = match finding {
                Value::String(text) => (text.clone(), String::new()),
                other => (
                    other
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    other
                        .get("explanation")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
            };
            format!("## {summary}\n\n{explanation}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!("# {title}\n\n{summary}\n\n{sections}")
}

/// `_pack_single_community_by_sub_communities` (`_op.py:417-461`).
fn pack_by_sub_communities(
    community: &CommunitySchema,
    max_token_size: usize,
    already_reports: &BTreeMap<i64, CommunitySchema>,
    tokenizer: &Tokenizer,
) -> (String, usize, HashSet<String>, HashSet<(String, String)>) {
    let mut sub_communities: Vec<&CommunitySchema> = community
        .sub_communities
        .iter()
        .filter_map(|key| key.parse::<i64>().ok())
        .filter_map(|key| already_reports.get(&key))
        .collect();
    sub_communities.sort_by(|a, b| {
        b.occurrence
            .partial_cmp(&a.occurrence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let sub_rows: Vec<&CommunitySchema> = truncate_list_by_token_size(
        &sub_communities,
        |community| community.report_string.clone().unwrap_or_default(),
        max_token_size,
        tokenizer,
    );

    let sub_fields = ["id", "report", "rating", "importance"];
    let mut rows = vec![sub_fields.iter().map(|s| s.to_string()).collect::<Vec<_>>()];
    for (index, community) in sub_rows.iter().enumerate() {
        let rating = community
            .report_json
            .as_ref()
            .and_then(|report| report.get("rating"))
            .map(|rating| match rating {
                Value::Number(number) => number.to_string(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "-1".to_string());
        rows.push(vec![
            index.to_string(),
            community.report_string.clone().unwrap_or_default(),
            rating,
            community.occurrence.to_string(),
        ]);
    }
    let describe = list_of_list_to_csv(&rows);
    let size = tokenizer.token_len(&describe);

    let mut nodes = HashSet::new();
    let mut edges = HashSet::new();
    for community in sub_rows {
        nodes.extend(community.nodes.iter().cloned());
        for (src, tgt) in &community.edges {
            edges.insert((src.clone(), tgt.clone()));
        }
    }
    (describe, size, nodes, edges)
}

/// `_pack_single_community_describe` (`_op.py:464-600`).
pub async fn pack_single_community_describe(
    graph: &dyn GraphStore,
    community: &CommunitySchema,
    tokenizer: &Tokenizer,
    max_token_size: usize,
    already_reports: &BTreeMap<i64, CommunitySchema>,
    force_to_use_sub_communities: bool,
) -> LlmResult<String> {
    let nodes_in_order = {
        let mut nodes = community.nodes.clone();
        nodes.sort();
        nodes
    };
    let edges_in_order = {
        let mut edges = community.edges.clone();
        edges.sort_by(|a, b| format!("{}{}", a.0, a.1).cmp(&format!("{}{}", b.0, b.1)));
        edges
    };

    let nodes_data = graph
        .get_nodes_batch(&nodes_in_order)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let edges_data = graph
        .get_edges_batch(&edges_in_order)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let final_template = "-----Reports-----\n```csv\n{reports}\n```\n-----Entities-----\n```csv\n{entities}\n```\n-----Relationships-----\n```csv\n{relationships}\n```";
    let base_template_tokens = tokenizer.token_len(
        &final_template
            .replace("{reports}", "")
            .replace("{entities}", "")
            .replace("{relationships}", ""),
    );
    let mut remaining_budget = max_token_size.saturating_sub(base_template_tokens);

    let truncated = nodes_in_order.len() > TRUNCATION_NODE_LIMIT
        || edges_in_order.len() > TRUNCATION_NODE_LIMIT;
    let need_sub_communities =
        truncated && !community.sub_communities.is_empty() && !already_reports.is_empty();

    let mut contain_nodes: HashSet<String> = HashSet::new();
    let mut contain_edges: HashSet<(String, String)> = HashSet::new();
    let mut report_describe = String::new();
    if need_sub_communities || force_to_use_sub_communities {
        let (describe, report_size, nodes, edges) =
            pack_by_sub_communities(community, remaining_budget, already_reports, tokenizer);
        report_describe = describe;
        remaining_budget = remaining_budget.saturating_sub(report_size);
        contain_nodes = nodes;
        contain_edges = edges;
    }

    let node_degrees = graph
        .node_degrees_batch(&nodes_in_order)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let edge_degrees = graph
        .edge_degrees_batch(&edges_in_order)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let mut node_rows: Vec<Vec<String>> = Vec::new();
    for (index, (name, data)) in nodes_in_order.iter().zip(nodes_data.iter()).enumerate() {
        if contain_nodes.contains(name) {
            continue;
        }
        let data = data.clone().unwrap_or_else(|| json!({}));
        node_rows.push(vec![
            index.to_string(),
            name.clone(),
            data.get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            data.get("description")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            node_degrees[index].to_string(),
        ]);
    }
    let mut edge_rows: Vec<Vec<String>> = Vec::new();
    for (index, ((src, tgt), data)) in edges_in_order.iter().zip(edges_data.iter()).enumerate() {
        if contain_edges.contains(&(src.clone(), tgt.clone())) {
            continue;
        }
        let data = data.clone().unwrap_or_else(|| json!({}));
        edge_rows.push(vec![
            index.to_string(),
            src.clone(),
            tgt.clone(),
            data.get("description")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            edge_degrees[index].to_string(),
        ]);
    }

    node_rows.sort_by_key(|row| std::cmp::Reverse(row[4].parse::<i64>().unwrap_or(0)));
    edge_rows.sort_by_key(|row| std::cmp::Reverse(row[4].parse::<i64>().unwrap_or(0)));

    let header_tokens = tokenizer.token_len(&format!(
        "{}\n{}",
        list_of_list_to_csv(&[vec![
            "id".to_string(),
            "entity".to_string(),
            "type".to_string(),
            "description".to_string(),
            "degree".to_string()
        ]]),
        list_of_list_to_csv(&[vec![
            "id".to_string(),
            "source".to_string(),
            "target".to_string(),
            "description".to_string(),
            "rank".to_string()
        ]])
    ));

    let data_budget = remaining_budget.saturating_sub(header_tokens);
    let total_items = node_rows.len() + edge_rows.len();
    let node_ratio = if total_items == 0 {
        0.0
    } else {
        node_rows.len() as f64 / total_items as f64
    };
    let edge_ratio = 1.0 - node_ratio;

    let nodes_budget = (data_budget as f64 * node_ratio) as usize;
    let edges_budget = (data_budget as f64 * edge_ratio) as usize;
    let row_text = |row: &Vec<String>| row.join(",");
    let nodes_final = truncate_list_by_token_size(&node_rows, row_text, nodes_budget, tokenizer);
    let edges_final = truncate_list_by_token_size(&edge_rows, row_text, edges_budget, tokenizer);

    let mut entity_rows = vec![vec![
        "id".to_string(),
        "entity".to_string(),
        "type".to_string(),
        "description".to_string(),
        "degree".to_string(),
    ]];
    entity_rows.extend(nodes_final);
    let mut relation_rows = vec![vec![
        "id".to_string(),
        "source".to_string(),
        "target".to_string(),
        "description".to_string(),
        "rank".to_string(),
    ]];
    relation_rows.extend(edges_final);

    Ok(final_template
        .replace("{reports}", &report_describe)
        .replace("{entities}", &list_of_list_to_csv(&entity_rows))
        .replace("{relationships}", &list_of_list_to_csv(&relation_rows)))
}

/// `generate_community_report` (`_op.py:625-697`): fill `report_string` and
/// `report_json` for every community, coarsest level first.
///
/// Two phases per level: descriptors are built sequentially (local reads), then
/// the LLM calls run concurrently under the limiter.
pub async fn generate_community_report(
    graph: &dyn GraphStore,
    communities: &[(i64, CommunitySchema)],
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &ReportOptions,
    max_async: usize,
) -> LlmResult<Vec<(i64, CommunitySchema)>> {
    let prompt_overhead = tokenizer.token_len(
        &fill_template(COMMUNITY_REPORT, &[("input_text", "")])
            .map_err(|e| LlmError::Decode(e.to_string()))?,
    );
    let max_token_size = options
        .best_model_max_token_size
        .saturating_sub(prompt_overhead)
        .saturating_sub(REPORT_PROMPT_HEADROOM);

    let mut reports: BTreeMap<i64, CommunitySchema> = BTreeMap::new();
    let mut levels: Vec<i64> = communities
        .iter()
        .map(|(_, community)| community.level)
        .collect();
    levels.sort_unstable();
    levels.dedup();
    levels.reverse(); // coarsest first

    let limiter = crate::core::concurrency::Limiter::new(max_async);
    for level in levels {
        // Phase A: prompts, sequentially (sub-community reports of coarser
        // levels are already available).
        let mut prompts: Vec<(i64, String)> = Vec::new();
        for (key, community) in communities.iter().filter(|(_, c)| c.level == level) {
            let describe = pack_single_community_describe(
                graph,
                community,
                tokenizer,
                max_token_size,
                &reports,
                false,
            )
            .await?;
            let prompt = fill_template(COMMUNITY_REPORT, &[("input_text", describe.as_str())])
                .map_err(|e| LlmError::Decode(e.to_string()))?;
            prompts.push((*key, prompt));
        }

        // Phase B: LLM calls, concurrently.
        let mut set: JoinSet<(i64, LlmResult<Value>)> = JoinSet::new();
        for (key, prompt) in prompts {
            let llm = llm.clone();
            let limiter = limiter.clone();
            let json_object = options.json_object;
            set.spawn(async move {
                let result = limiter
                    .run(llm.complete_cached(
                        &prompt,
                        None,
                        &[],
                        &ModelOptions {
                            max_tokens: None,
                            json_object,
                        },
                    ))
                    .await;
                let parsed = match result {
                    Ok((response, _)) => convert_response_to_json(&response)
                        .unwrap_or_else(|| json!({"title": "Report"})),
                    Err(err) => return (key, Err(err)),
                };
                (key, Ok(parsed))
            });
        }

        // Phase C: assemble.
        let mut level_reports: Vec<(i64, Value)> = Vec::new();
        while let Some(joined) = set.join_next().await {
            let (key, result) = joined.map_err(|e| LlmError::Transport(format!("join: {e}")))?;
            level_reports.push((key, result?));
        }
        let mut by_key: BTreeMap<i64, Value> = level_reports.into_iter().collect();
        for (key, community) in communities.iter().filter(|(_, c)| c.level == level) {
            let parsed = by_key
                .remove(key)
                .unwrap_or_else(|| json!({"title": "Report"}));
            let mut filled = community.clone();
            filled.report_string = Some(community_report_json_to_str(&parsed));
            filled.report_json = Some(parsed);
            reports.insert(*key, filled);
        }
    }

    Ok(reports.into_iter().collect())
}

/// KV rows for the community report store (`community_reports` namespace):
/// `{ "report_string": ..., "report_json": ..., level, title, edges, nodes,
/// chunk_ids, occurrence, sub_communities }`.
pub fn report_kv_rows(communities: &[(i64, CommunitySchema)]) -> Vec<(String, Value)> {
    communities
        .iter()
        .map(|(key, community)| {
            let mut row = serde_json::to_value(community).expect("community serializes");
            row["report_string"] = json!(community.report_string.clone().unwrap_or_default());
            row["report_json"] = community.report_json.clone().unwrap_or(Value::Null);
            (key.to_string(), row)
        })
        .collect()
}

/// `rating` used for ordering in global search (`_op.py:1046`).
pub fn report_rating(community: &CommunitySchema) -> f64 {
    community
        .report_json
        .as_ref()
        .and_then(|report| report.get("rating"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}
