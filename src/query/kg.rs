//! `kg_query` — the unified keyword-arm search (`operate.py:4693`).
//!
//! Flow: extract keywords → empty-keyword rules → per-mode arm selection
//! (local = ll arm, global = hl arm, hybrid = both, mix = both + chunk arm) →
//! round-robin dedup merge of entities and relations → token truncation →
//! chunk merge → context render → answer.
//!
//! Both arms expand exactly one hop: the entity arm takes the incident edges of
//! the matched entities, the relation arm takes the endpoints of the matched
//! relations. There is no multi-hop traversal in the reference
//! (`_get_node_data`, operate.py:6200).

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::core::rag::{QueryMode, QueryParam};
use crate::core::text::{truncate_list_by_token_size, GRAPH_FIELD_SEP};
use crate::core::traits::VectorStore;
use crate::graph::prompts::{FAIL_RESPONSE, RAG_RESPONSE};
use crate::llm::{LlmError, LlmResult, ModelOptions};
use crate::query::context::{
    assign_chunk_ids, format_created_at, reference_list, ChunkRow, EntityRow, QueryContext,
    RelationRow,
};
use crate::query::fusion::fuse_chunks;
use crate::query::keywords::extract_keywords_only;
use crate::query::QueryStores;

/// `kg_query` (`operate.py:4693`). `Ok(None)` matches the reference's
/// "no query context could be built" return.
pub async fn kg_query(
    stores: &QueryStores,
    query: &str,
    param: &QueryParam,
) -> LlmResult<Option<String>> {
    if query.is_empty() {
        return Ok(Some(FAIL_RESPONSE.to_string()));
    }

    let keywords = extract_keywords_only(&stores.llm, query).await?;
    let high = keywords.high;
    let mut low = keywords.low;

    // Empty-keyword rules (`operate.py:4755-4764`).
    if high.is_empty() && low.is_empty() {
        if query.len() < 50 {
            low.push(query.to_string());
        } else {
            return Ok(Some(FAIL_RESPONSE.to_string()));
        }
    }

    let high_joined = high.join(", ");
    let low_joined = low.join(", ");
    let need_ll = matches!(
        param.mode,
        QueryMode::Local | QueryMode::Hybrid | QueryMode::Mix
    ) && !low.is_empty();
    let need_hl = matches!(
        param.mode,
        QueryMode::Global | QueryMode::Hybrid | QueryMode::Mix
    ) && !high.is_empty();

    // Stage 1: the two keyword arms.
    let (local_entities, local_relations) = if need_ll {
        entity_arm(stores, &low_joined, param).await?
    } else {
        (Vec::new(), Vec::new())
    };
    let (global_entities, global_relations) = if need_hl {
        relation_arm(stores, &high_joined, param).await?
    } else {
        (Vec::new(), Vec::new())
    };

    // Round-robin merge with dedup (`operate.py:5429-5470`).
    let mut entities = round_robin(local_entities, global_entities, |row| {
        row.get("entity_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let mut relations = round_robin(local_relations, global_relations, relation_key);

    // Stage 2: token truncation. `created_at`/`file_path` are excluded from the
    // measured text, exactly like the reference.
    entities = truncate_entities(entities, param, &stores.tokenizer);
    relations = truncate_relations(relations, param, &stores.tokenizer);
    if entities.is_empty() && relations.is_empty() {
        return Ok(None);
    }

    // Stage 3: chunk merge — entity chunks, relation chunks, and (mix) the
    // vector arm.
    let mut chunks: Vec<ChunkRow> = Vec::new();
    let mut vector_chunks: Vec<ChunkRow> = Vec::new();
    if param.mode == QueryMode::Mix {
        if let Some(chunks_vdb) = &stores.chunks_vdb {
            vector_chunks = chunk_hits(stores, chunks_vdb, query, param).await?;
        }
    }
    let entity_chunks = chunks_for_elements(stores, &entities).await?;
    let relation_chunks = chunks_for_elements(stores, &relations).await?;
    chunks.extend(vector_chunks.iter().cloned());
    chunks.extend(entity_chunks);
    chunks.extend(relation_chunks);
    let chunks = fuse_chunks(chunks, query, param, &stores.tokenizer, None);

    if chunks.is_empty() && entities.is_empty() && relations.is_empty() {
        return Ok(None);
    }

    // Stage 4: render.
    let context = build_context(&entities, &relations, &chunks);
    let rendered = context
        .render()
        .map_err(|e| LlmError::Decode(format!("context render: {e}")))?;
    if param.only_need_context {
        return Ok(Some(rendered));
    }

    let system_prompt = crate::core::text::fill_template(
        RAG_RESPONSE,
        &[
            ("context_data", rendered.as_str()),
            ("response_type", param.response_type.as_str()),
        ],
    )
    .map_err(|e| LlmError::Decode(format!("rag_response template: {e}")))?;
    let (answer, _) = stores
        .llm
        .complete_cached_keyed(
            query,
            Some(&system_prompt),
            &[],
            &ModelOptions::default(),
            "query",
        )
        .await?;
    Ok(Some(answer))
}

/// The low-level (entity) arm: vector hit → node data → one hop of incident
/// edges (`_get_node_data`, operate.py:6200-6519).
async fn entity_arm(
    stores: &QueryStores,
    ll_keywords: &str,
    param: &QueryParam,
) -> LlmResult<(Vec<Value>, Vec<Value>)> {
    let hits = stores
        .entities_vdb
        .query(ll_keywords, param.top_k)
        .await
        .map_err(|e| LlmError::Transport(format!("entity search: {e}")))?;
    if hits.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let names: Vec<String> = hits
        .iter()
        .filter_map(|hit| {
            hit.meta
                .get("entity_name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let nodes = stores
        .graph
        .get_nodes_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let mut entities: Vec<Value> = Vec::new();
    for (name, node) in names.iter().zip(nodes) {
        let Some(node) = node else { continue };
        let mut row = node.clone();
        if let Some(object) = row.as_object_mut() {
            object.insert("entity_name".to_string(), json!(name));
        }
        entities.push(row);
    }

    // One hop: the incident edges of the matched entities.
    let edges = stores
        .graph
        .nodes_edges_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let mut pairs: BTreeSet<(String, String)> = BTreeSet::new();
    for entry in edges.into_iter().flatten() {
        for (src, tgt) in entry {
            let key = if src <= tgt { (src, tgt) } else { (tgt, src) };
            pairs.insert(key);
        }
    }
    let mut relations: Vec<Value> = Vec::new();
    for (src, tgt) in pairs {
        if let Some(edge) = stores
            .graph
            .get_edge(&src, &tgt)
            .await
            .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?
        {
            let mut row = edge;
            if let Some(object) = row.as_object_mut() {
                object.insert("src_id".to_string(), json!(src));
                object.insert("tgt_id".to_string(), json!(tgt));
            }
            relations.push(row);
        }
    }
    Ok((entities, relations))
}

/// The high-level (relation) arm: vector hit → edge data → endpoints
/// (`_get_edge_data`, operate.py:6529-6602).
async fn relation_arm(
    stores: &QueryStores,
    hl_keywords: &str,
    param: &QueryParam,
) -> LlmResult<(Vec<Value>, Vec<Value>)> {
    let Some(relationships_vdb) = &stores.relationships_vdb else {
        return Ok((Vec::new(), Vec::new()));
    };
    let hits = relationships_vdb
        .query(hl_keywords, param.top_k)
        .await
        .map_err(|e| LlmError::Transport(format!("relation search: {e}")))?;
    if hits.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut relations: Vec<Value> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for hit in &hits {
        let (Some(src), Some(tgt)) = (
            hit.meta.get("src_id").and_then(Value::as_str),
            hit.meta.get("tgt_id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let Some(edge) = stores
            .graph
            .get_edge(src, tgt)
            .await
            .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?
        else {
            continue;
        };
        let mut row = edge;
        if let Some(object) = row.as_object_mut() {
            object.insert("src_id".to_string(), json!(src));
            object.insert("tgt_id".to_string(), json!(tgt));
        }
        relations.push(row);
        for endpoint in [src, tgt] {
            if !names.iter().any(|name| name == endpoint) {
                names.push(endpoint.to_string());
            }
        }
    }
    let nodes = stores
        .graph
        .get_nodes_batch(&names)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;
    let mut entities: Vec<Value> = Vec::new();
    for (name, node) in names.iter().zip(nodes) {
        let Some(node) = node else { continue };
        let mut row = node;
        if let Some(object) = row.as_object_mut() {
            object.insert("entity_name".to_string(), json!(name));
        }
        entities.push(row);
    }
    Ok((entities, relations))
}

/// Interleave two lists, first occurrence wins per key (`operate.py:5429`).
fn round_robin<T>(first: Vec<T>, second: Vec<T>, key: impl Fn(&T) -> String) -> Vec<T> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<T> = Vec::new();
    let mut first = first.into_iter();
    let mut second = second.into_iter();
    loop {
        let mut progressed = false;
        if let Some(row) = first.next() {
            progressed = true;
            if seen.insert(key(&row)) {
                out.push(row);
            }
        }
        if let Some(row) = second.next() {
            progressed = true;
            if seen.insert(key(&row)) {
                out.push(row);
            }
        }
        if !progressed {
            return out;
        }
    }
}

fn relation_key(row: &Value) -> String {
    let src = row
        .get("src_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tgt = row
        .get("tgt_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut pair = [src, tgt];
    pair.sort();
    pair.join(GRAPH_FIELD_SEP)
}

fn truncate_entities(
    rows: Vec<Value>,
    param: &QueryParam,
    tokenizer: &crate::core::text::Tokenizer,
) -> Vec<Value> {
    let measured: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut copy = row.clone();
            if let Some(object) = copy.as_object_mut() {
                object.remove("created_at");
                object.remove("file_path");
            }
            copy
        })
        .collect();
    let kept = truncate_list_by_token_size(
        &measured,
        |row| row.to_string(),
        param.max_entity_tokens,
        tokenizer,
    );
    kept.into_iter()
        .filter_map(|row| {
            let name = row.get("entity_name").and_then(Value::as_str)?.to_string();
            rows.iter()
                .find(|original| {
                    original.get("entity_name").and_then(Value::as_str) == Some(name.as_str())
                })
                .cloned()
        })
        .collect()
}

fn truncate_relations(
    rows: Vec<Value>,
    param: &QueryParam,
    tokenizer: &crate::core::text::Tokenizer,
) -> Vec<Value> {
    let measured: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut copy = row.clone();
            if let Some(object) = copy.as_object_mut() {
                object.remove("created_at");
                object.remove("file_path");
            }
            copy
        })
        .collect();
    let kept = truncate_list_by_token_size(
        &measured,
        |row| row.to_string(),
        param.max_relation_tokens,
        tokenizer,
    );
    kept.into_iter()
        .filter_map(|row| {
            let key = relation_key(&row);
            rows.iter()
                .find(|original| relation_key(original) == key)
                .cloned()
        })
        .collect()
}

/// The chunk rows one set of elements points at, via the tracking stores.
async fn chunks_for_elements(stores: &QueryStores, rows: &[Value]) -> LlmResult<Vec<ChunkRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut chunk_ids: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let ids: Vec<String> = row
            .get("source_id")
            .and_then(Value::as_str)
            .map(|value| {
                value
                    .split(GRAPH_FIELD_SEP)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        chunk_ids.extend(ids);
    }
    let ids: Vec<String> = chunk_ids.into_iter().collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys = stores
        .text_chunks
        .get_by_ids(&ids)
        .await
        .map_err(|e| LlmError::Transport(format!("chunk read: {e}")))?;
    let mut out = Vec::new();
    for (id, row) in ids.into_iter().zip(keys) {
        let Some(row) = row else { continue };
        out.push(ChunkRow {
            id,
            content: row
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            file_path: row
                .get("file_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            chunk_order_index: row
                .get("chunk_order_index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        });
    }
    Ok(out)
}

async fn chunk_hits(
    stores: &QueryStores,
    chunks_vdb: &Arc<dyn VectorStore>,
    query: &str,
    param: &QueryParam,
) -> LlmResult<Vec<ChunkRow>> {
    let hits = chunks_vdb
        .query(query, param.chunk_top_k)
        .await
        .map_err(|e| LlmError::Transport(format!("chunk search: {e}")))?;
    let ids: Vec<String> = hits.iter().map(|hit| hit.id.clone()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = stores
        .text_chunks
        .get_by_ids(&ids)
        .await
        .map_err(|e| LlmError::Transport(format!("chunk read: {e}")))?;
    let mut out = Vec::new();
    for (id, row) in ids.into_iter().zip(rows) {
        let Some(row) = row else { continue };
        out.push(ChunkRow {
            id,
            content: row
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            file_path: row
                .get("file_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            chunk_order_index: 0,
        });
    }
    Ok(out)
}

fn build_context(entities: &[Value], relations: &[Value], chunks: &[ChunkRow]) -> QueryContext {
    let entity_rows = entities
        .iter()
        .map(|row| EntityRow {
            entity: row
                .get("entity_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            entity_type: row
                .get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            description: row
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            created_at: format_created_at(
                row.get("created_at").and_then(Value::as_i64).unwrap_or(0),
            ),
            file_path: row
                .get("file_path")
                .and_then(Value::as_str)
                .unwrap_or("unknown_source")
                .to_string(),
        })
        .collect();
    let relation_rows = relations
        .iter()
        .map(|row| RelationRow {
            entity1: row
                .get("src_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            entity2: row
                .get("tgt_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            description: row
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            created_at: format_created_at(
                row.get("created_at").and_then(Value::as_i64).unwrap_or(0),
            ),
            file_path: row
                .get("file_path")
                .and_then(Value::as_str)
                .unwrap_or("unknown_source")
                .to_string(),
        })
        .collect();
    let chunk_rows = assign_chunk_ids(chunks.to_vec());
    let references = reference_list(&chunk_rows);
    QueryContext {
        entities: entity_rows,
        relations: relation_rows,
        chunks: chunk_rows,
        references,
    }
}
