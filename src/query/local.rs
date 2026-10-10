//! Local search — port of `_op.py` `_find_most_related_community_from_entities`
//! (700-745), `_find_most_related_text_unit_from_entities` (748-804),
//! `_find_most_related_edges_from_entities` (807-841),
//! `_build_local_query_context` (844-932) and `local_query` (935-967).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::{json, Value};

use crate::core::rag::{CommunitySchema, QueryParam};
use crate::core::text::{
    list_of_list_to_csv, truncate_list_by_token_size, CsvCell, GRAPH_FIELD_SEP,
};
use crate::graph::prompts::{FAIL_RESPONSE, LOCAL_RAG_RESPONSE};
use crate::llm::{LlmError, LlmResult, ModelOptions};
use crate::query::QueryStores;

/// `local_query` (`_op.py:935-967`).
pub async fn local_query(
    stores: &QueryStores,
    query: &str,
    param: &QueryParam,
) -> LlmResult<String> {
    let context = build_local_query_context(stores, query, param).await?;
    if param.only_need_context {
        return Ok(context.unwrap_or_default());
    }
    let context = match context {
        Some(context) => context,
        None => return Ok(FAIL_RESPONSE.to_string()),
    };
    let system_prompt = crate::core::text::fill_template(
        LOCAL_RAG_RESPONSE,
        &[
            ("context_data", context.as_str()),
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

/// `_build_local_query_context` (`_op.py:844-932`).
pub async fn build_local_query_context(
    stores: &QueryStores,
    query: &str,
    param: &QueryParam,
) -> LlmResult<Option<String>> {
    let hits = stores
        .entities_vdb
        .query(query, param.top_k)
        .await
        .map_err(|e| LlmError::Transport(format!("entity search: {e}")))?;
    if hits.is_empty() {
        return Ok(None);
    }

    let names: Vec<String> = hits
        .iter()
        .map(|hit| {
            hit.meta
                .get("entity_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let nodes = stores
        .graph
        .get_nodes_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let degrees = stores
        .graph
        .node_degrees_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let mut node_datas: Vec<Value> = Vec::new();
    for ((name, data), degree) in names.iter().zip(nodes.iter()).zip(degrees.iter()) {
        if let Some(data) = data {
            let mut row = data.clone();
            row["entity_name"] = json!(name);
            row["rank"] = json!(degree);
            node_datas.push(row);
        }
    }
    if node_datas.is_empty() {
        return Ok(None);
    }

    let communities = most_related_communities(stores, &node_datas, param).await?;
    let text_units = most_related_text_units(stores, &node_datas, param).await?;
    let relations = most_related_edges(stores, &node_datas, param).await?;

    let mut entity_rows = vec![vec![
        CsvCell::text("id"),
        CsvCell::text("entity"),
        CsvCell::text("type"),
        CsvCell::text("description"),
        CsvCell::text("rank"),
    ]];
    for (index, node) in node_datas.iter().enumerate() {
        entity_rows.push(vec![
            CsvCell::int(index as i64),
            CsvCell::text(node["entity_name"].as_str().unwrap_or("UNKNOWN")),
            CsvCell::text(node["entity_type"].as_str().unwrap_or("UNKNOWN")),
            CsvCell::text(node["description"].as_str().unwrap_or("UNKNOWN")),
            CsvCell::int(node["rank"].as_i64().unwrap_or_default()),
        ]);
    }
    let mut relation_rows = vec![vec![
        CsvCell::text("id"),
        CsvCell::text("source"),
        CsvCell::text("target"),
        CsvCell::text("description"),
        CsvCell::text("weight"),
        CsvCell::text("rank"),
    ]];
    for (index, relation) in relations.iter().enumerate() {
        relation_rows.push(vec![
            CsvCell::int(index as i64),
            CsvCell::text(relation["src_tgt"][0].as_str().unwrap_or_default()),
            CsvCell::text(relation["src_tgt"][1].as_str().unwrap_or_default()),
            CsvCell::text(relation["description"].as_str().unwrap_or("UNKNOWN")),
            CsvCell::float(relation["weight"].as_f64().unwrap_or_default()),
            CsvCell::int(relation["rank"].as_i64().unwrap_or_default()),
        ]);
    }
    let mut community_rows = vec![vec![CsvCell::text("id"), CsvCell::text("content")]];
    for (index, community) in communities.iter().enumerate() {
        community_rows.push(vec![
            CsvCell::int(index as i64),
            CsvCell::text(community.report_string.clone().unwrap_or_default()),
        ]);
    }
    let mut text_rows = vec![vec![CsvCell::text("id"), CsvCell::text("content")]];
    for (index, unit) in text_units.iter().enumerate() {
        text_rows.push(vec![
            CsvCell::int(index as i64),
            CsvCell::text(unit["content"].as_str().unwrap_or_default()),
        ]);
    }

    Ok(Some(format!(
        "\n-----Reports-----\n```csv\n{}\n```\n-----Entities-----\n```csv\n{}\n```\n-----Relationships-----\n```csv\n{}\n```\n-----Sources-----\n```csv\n{}\n```\n",
        list_of_list_to_csv(&community_rows),
        list_of_list_to_csv(&entity_rows),
        list_of_list_to_csv(&relation_rows),
        list_of_list_to_csv(&text_rows),
    )))
}

/// `_find_most_related_community_from_entities` (`_op.py:700-745`).
async fn most_related_communities(
    stores: &QueryStores,
    node_datas: &[Value],
    param: &QueryParam,
) -> LlmResult<Vec<CommunitySchema>> {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    let mut order: Vec<i64> = Vec::new();
    for node in node_datas {
        let Some(clusters) = node.get("clusters") else {
            continue;
        };
        let Ok(clusters) = serde_json::from_value::<Vec<Value>>(clusters.clone()) else {
            continue;
        };
        for cluster in clusters {
            let level = cluster.get("level").and_then(Value::as_i64).unwrap_or(0);
            if level > param.level {
                continue;
            }
            let Some(key) = cluster.get("cluster").and_then(Value::as_i64) else {
                continue;
            };
            if !counts.contains_key(&key) {
                order.push(key);
            }
            *counts.entry(key).or_insert(0) += 1;
        }
    }

    let keys: Vec<String> = order.iter().map(|key| key.to_string()).collect();
    let rows = stores
        .community_reports
        .get_by_ids(&keys)
        .await
        .map_err(|e| LlmError::Transport(format!("report read: {e}")))?;
    let mut with_reports: Vec<(i64, CommunitySchema)> = Vec::new();
    for (key, row) in order.iter().zip(rows.iter()) {
        if let Some(row) = row {
            if let Ok(community) = serde_json::from_value::<CommunitySchema>(row.clone()) {
                with_reports.push((*key, community));
            }
        }
    }
    with_reports.sort_by(|a, b| {
        let count_a = counts[&a.0];
        let count_b = counts[&b.0];
        let rating_a = crate::graph::reports::report_rating(&a.1);
        let rating_b = crate::graph::reports::report_rating(&b.1);
        count_b.cmp(&count_a).then_with(|| {
            rating_b
                .partial_cmp(&rating_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    let mut communities: Vec<CommunitySchema> = with_reports
        .into_iter()
        .map(|(_, community)| community)
        .collect();
    communities = truncate_list_by_token_size(
        &communities,
        |community| community.report_string.clone().unwrap_or_default(),
        param.local_max_token_for_community_report,
        &stores.tokenizer,
    );
    if param.local_community_single_one {
        communities.truncate(1);
    }
    Ok(communities)
}

/// `_find_most_related_text_unit_from_entities` (`_op.py:748-804`).
async fn most_related_text_units(
    stores: &QueryStores,
    node_datas: &[Value],
    param: &QueryParam,
) -> LlmResult<Vec<Value>> {
    let text_unit_ids: Vec<Vec<String>> = node_datas
        .iter()
        .map(|node| {
            crate::core::text::split_string_by_multi_markers(
                node["source_id"].as_str().unwrap_or_default(),
                &[GRAPH_FIELD_SEP],
            )
        })
        .collect();
    let names: Vec<String> = node_datas
        .iter()
        .map(|node| node["entity_name"].as_str().unwrap_or_default().to_string())
        .collect();
    let node_edges = stores
        .graph
        .nodes_edges_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    // One-hop neighbours, first-encounter order.
    let mut one_hop: Vec<String> = Vec::new();
    let mut seen_one_hop: HashSet<String> = HashSet::new();
    for edges in node_edges.iter().flatten() {
        for (_, other) in edges {
            if seen_one_hop.insert(other.clone()) {
                one_hop.push(other.clone());
            }
        }
    }
    let one_hop_data = stores
        .graph
        .get_nodes_batch(&one_hop)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let mut one_hop_lookup: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, data) in one_hop.iter().zip(one_hop_data.iter()) {
        if let Some(data) = data {
            let ids = crate::core::text::split_string_by_multi_markers(
                data["source_id"].as_str().unwrap_or_default(),
                &[GRAPH_FIELD_SEP],
            );
            one_hop_lookup.insert(name.clone(), ids.into_iter().collect());
        }
    }

    let mut lookup: Vec<(String, usize, usize)> = Vec::new(); // (chunk id, order, relation count)
    let mut seen: HashSet<String> = HashSet::new();
    for (index, (units, edges)) in text_unit_ids.iter().zip(node_edges.iter()).enumerate() {
        let edges = edges.clone().unwrap_or_default();
        for chunk_id in units {
            if !seen.insert(chunk_id.clone()) {
                continue;
            }
            let mut relation_counts = 0usize;
            for (_, other) in &edges {
                if let Some(other_units) = one_hop_lookup.get(other) {
                    if other_units.contains(chunk_id) {
                        relation_counts += 1;
                    }
                }
            }
            lookup.push((chunk_id.clone(), index, relation_counts));
        }
    }
    lookup.sort_by_key(|entry| (entry.1, std::cmp::Reverse(entry.2)));

    let ids: Vec<String> = lookup.iter().map(|(id, _, _)| id.clone()).collect();
    let rows = stores
        .text_chunks
        .get_by_ids(&ids)
        .await
        .map_err(|e| LlmError::Transport(format!("chunk read: {e}")))?;
    let mut units: Vec<(String, Value)> = Vec::new();
    for ((id, _, _), row) in lookup.iter().zip(rows.iter()) {
        if let Some(row) = row {
            if !row.is_null() {
                units.push((id.clone(), row.clone()));
            }
        }
    }
    let truncated = truncate_list_by_token_size(
        &units,
        |(_, row)| row["content"].as_str().unwrap_or_default().to_string(),
        param.local_max_token_for_text_unit,
        &stores.tokenizer,
    );
    Ok(truncated.into_iter().map(|(_, row)| row).collect())
}

/// `_find_most_related_edges_from_entities` (`_op.py:807-841`).
async fn most_related_edges(
    stores: &QueryStores,
    node_datas: &[Value],
    param: &QueryParam,
) -> LlmResult<Vec<Value>> {
    let names: Vec<String> = node_datas
        .iter()
        .map(|node| node["entity_name"].as_str().unwrap_or_default().to_string())
        .collect();
    let node_edges = stores
        .graph
        .nodes_edges_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for edges in node_edges.iter().flatten() {
        for (a, b) in edges {
            let sorted = if a <= b {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            };
            if seen.insert(sorted.clone()) {
                pairs.push(sorted);
            }
        }
    }
    let edge_data = stores
        .graph
        .get_edges_batch(&pairs)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let edge_degrees = stores
        .graph
        .edge_degrees_batch(&pairs)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let mut rows: Vec<Value> = Vec::new();
    for ((pair, data), degree) in pairs.iter().zip(edge_data.iter()).zip(edge_degrees.iter()) {
        if let Some(data) = data {
            let mut row = data.clone();
            row["src_tgt"] = json!([pair.0, pair.1]);
            row["rank"] = json!(degree);
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| {
        let rank_a = a["rank"].as_i64().unwrap_or(0);
        let rank_b = b["rank"].as_i64().unwrap_or(0);
        let weight_a = a["weight"].as_f64().unwrap_or(0.0);
        let weight_b = b["weight"].as_f64().unwrap_or(0.0);
        rank_b.cmp(&rank_a).then_with(|| {
            weight_b
                .partial_cmp(&weight_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    Ok(truncate_list_by_token_size(
        &rows,
        |row| row["description"].as_str().unwrap_or_default().to_string(),
        param.local_max_token_for_local_context,
        &stores.tokenizer,
    ))
}
