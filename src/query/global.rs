//! Global search — port of `_op.py` `_map_global_communities` (970-1014) and
//! `global_query` (1017-1104): level filter, occurrence ordering, rating
//! filter, map/reduce over community reports.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tokio::task::JoinSet;

use crate::core::concurrency::Limiter;
use crate::core::rag::{CommunitySchema, QueryParam};
use crate::core::text::{list_of_list_to_csv, truncate_list_by_token_size};
use crate::core::traits::GraphSnapshot;
use crate::graph::community::{build_schema_from_snapshot, ClusterRef};
use crate::graph::prompts::{FAIL_RESPONSE, GLOBAL_MAP_RAG_POINTS, GLOBAL_REDUCE_RAG_RESPONSE};
use crate::graph::reports::report_rating;
use crate::llm::{LlmError, LlmResult, ModelOptions};
use crate::query::QueryStores;

/// `global_query` (`_op.py:1017-1104`).
pub async fn global_query(
    stores: &QueryStores,
    query: &str,
    param: &QueryParam,
) -> LlmResult<String> {
    let snapshot = stores
        .graph
        .snapshot()
        .await
        .map_err(|e| LlmError::Transport(format!("graph snapshot: {e}")))?;
    let memberships = memberships_from_snapshot(&snapshot);
    let schema = build_schema_from_snapshot(&snapshot, &memberships);
    let mut schema: Vec<(i64, CommunitySchema)> = schema
        .into_iter()
        .filter(|(_, community)| community.level <= param.level)
        .collect();
    if schema.is_empty() {
        return Ok(FAIL_RESPONSE.to_string());
    }
    schema.sort_by(|a, b| {
        b.1.occurrence
            .partial_cmp(&a.1.occurrence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    schema.truncate(param.global_max_consider_community);

    let keys: Vec<String> = schema.iter().map(|(key, _)| key.to_string()).collect();
    let rows = stores
        .community_reports
        .get_by_ids(&keys)
        .await
        .map_err(|e| LlmError::Transport(format!("report read: {e}")))?;
    let mut communities: Vec<CommunitySchema> = Vec::new();
    for row in rows.iter().flatten() {
        if let Ok(community) = serde_json::from_value::<CommunitySchema>(row.clone()) {
            if report_rating(&community) >= param.global_min_community_rating {
                communities.push(community);
            }
        }
    }
    communities.sort_by(|a, b| {
        a.occurrence
            .partial_cmp(&b.occurrence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                report_rating(a)
                    .partial_cmp(&report_rating(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    communities.reverse();

    let points = map_communities(stores, query, &communities, param).await?;
    let mut support: Vec<(usize, String, f64)> = Vec::new();
    for (analyst, group) in points.iter().enumerate() {
        let Some(points) = group.get("points").and_then(Value::as_array).cloned() else {
            continue;
        };
        for point in points {
            let Some(description) = point.get("description").and_then(Value::as_str) else {
                continue;
            };
            let score = point.get("score").and_then(Value::as_f64).unwrap_or(1.0);
            support.push((analyst, description.to_string(), score));
        }
    }
    support.retain(|(_, _, score)| *score > 0.0);
    if support.is_empty() {
        return Ok(FAIL_RESPONSE.to_string());
    }
    support.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let support = truncate_list_by_token_size(
        &support,
        |(_, answer, _)| answer.clone(),
        param.global_max_token_for_community_report,
        &stores.tokenizer,
    );
    let context = support
        .iter()
        .map(|(analyst, answer, score)| {
            format!("----Analyst {analyst}----\nImportance Score: {score}\n{answer}\n")
        })
        .collect::<Vec<_>>()
        .join("\n");
    if param.only_need_context {
        return Ok(context);
    }
    let system_prompt = crate::core::text::fill_template(
        GLOBAL_REDUCE_RAG_RESPONSE,
        &[
            ("report_data", context.as_str()),
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

/// `_map_global_communities` (`_op.py:970-1014`): group reports by the token
/// budget and ask the model for support points per group.
async fn map_communities(
    stores: &QueryStores,
    query: &str,
    communities: &[CommunitySchema],
    param: &QueryParam,
) -> LlmResult<Vec<Value>> {
    let mut groups: Vec<Vec<CommunitySchema>> = Vec::new();
    let mut remaining: &[CommunitySchema] = communities;
    while !remaining.is_empty() {
        let group = truncate_list_by_token_size(
            remaining,
            |community| community.report_string.clone().unwrap_or_default(),
            param.global_max_token_for_community_report,
            &stores.tokenizer,
        );
        // Guard against a zero progress loop when a single report exceeds the
        // budget (the reference slices by group length for the same reason).
        let taken = if group.is_empty() { 1 } else { group.len() };
        groups.push(remaining[..taken].to_vec());
        remaining = &remaining[taken..];
    }

    let limiter = Limiter::new(stores.max_async);
    let mut set: JoinSet<(usize, LlmResult<Value>)> = JoinSet::new();
    for (index, group) in groups.into_iter().enumerate() {
        let llm = stores.llm.clone();
        let limiter = limiter.clone();
        let query = query.to_string();
        set.spawn(async move {
            let mut rows = vec![vec![
                "id".to_string(),
                "content".to_string(),
                "rating".to_string(),
                "importance".to_string(),
            ]];
            for (position, community) in group.iter().enumerate() {
                rows.push(vec![
                    position.to_string(),
                    community.report_string.clone().unwrap_or_default(),
                    report_rating(community).to_string(),
                    community.occurrence.to_string(),
                ]);
            }
            let context = list_of_list_to_csv(&rows);
            let system_prompt = match crate::core::text::fill_template(
                GLOBAL_MAP_RAG_POINTS,
                &[("context_data", context.as_str())],
            ) {
                Ok(prompt) => prompt,
                Err(err) => return (index, Err(LlmError::Decode(err.to_string()))),
            };
            let result = limiter
                .run(llm.complete_cached(
                    &query,
                    Some(&system_prompt),
                    &[],
                    &ModelOptions {
                        max_tokens: None,
                        json_object: true,
                    },
                ))
                .await;
            match result {
                Ok((response, _)) => (
                    index,
                    Ok(crate::core::text::convert_response_to_json(&response).unwrap_or(json!({}))),
                ),
                Err(err) => (index, Err(err)),
            }
        });
    }

    let mut results: BTreeMap<usize, Value> = BTreeMap::new();
    while let Some(joined) = set.join_next().await {
        let (index, result) = joined.map_err(|e| LlmError::Transport(format!("join: {e}")))?;
        results.insert(index, result?);
    }
    Ok(results.into_values().collect())
}

/// Cluster memberships straight from the snapshot's `clusters` attributes.
pub fn memberships_from_snapshot(snapshot: &GraphSnapshot) -> Vec<(String, Vec<ClusterRef>)> {
    let mut out = Vec::new();
    for (name, data) in &snapshot.nodes {
        if let Some(clusters) = data.get("clusters") {
            if let Ok(parsed) = serde_json::from_value::<Vec<ClusterRef>>(clusters.clone()) {
                out.push((name.clone(), parsed));
            }
        }
    }
    out
}
