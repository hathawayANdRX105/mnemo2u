//! Incremental merge — LightRAG's `merge_nodes_and_edges` family.
//!
//! The reference's merge is document-scoped: Phase 0 writes the
//! `full_entities`/`full_relations` anchor rows for the document (so a crash
//! mid-run can be detected and re-driven), Phase 1 merges entities, Phase 2
//! merges relations — and only the elements the document actually mentions are
//! touched (`operate.py::merge_nodes_and_edges` :3514, called once per document
//! at `pipeline.py:5803`).
//!
//! The merge rules ported here (`operate.py::_merge_nodes_then_upsert` :2429,
//! `_merge_edges_then_upsert` :2782):
//!
//! - entity type: most common wins, ties keep first appearance;
//! - description: sorted-union of the old and new fragments, joined by
//!   `GRAPH_FIELD_SEP`, then the three-tier summary gate;
//! - `source_id`: first-seen-ordered dedup union;
//! - relation weight: stored weight plus the weights of edges whose source is
//!   not already stored (re-fed sources must not double-count), floored at the
//!   number of distinct evidence sources;
//! - relation keywords: sorted-union, comma-joined.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use serde_json::{json, Value};

use crate::core::rag::{EntityRecord, RelationRecord};
use crate::core::text::{
    compute_mdhash_id, py_repr_tuple2, sanitize_and_normalize_extracted_text,
    split_string_by_multi_markers, Tokenizer,
};
use crate::core::traits::{GraphStore, KvStore, VectorRow, VectorStore};
use crate::graph::prompts::{
    DEFAULT_FORCE_LLM_SUMMARY_ON_MERGE, DEFAULT_SUMMARY_CONTEXT_SIZE,
    DEFAULT_SUMMARY_LENGTH_RECOMMENDED, DEFAULT_SUMMARY_MAX_TOKENS, GRAPH_FIELD_SEP,
    SUMMARIZE_ENTITY_DESCRIPTIONS,
};
use crate::index::tracking::{make_relation_chunk_key, TrackingStores};
use crate::llm::cache::CachedLlm;
use crate::llm::{LlmError, LlmResult, ModelOptions};

/// KV namespace for the per-document anchor rows (Phase 0).
pub const NS_FULL_ENTITIES: &str = "full_entities";
/// KV namespace for the per-document relation anchor rows (Phase 0).
pub const NS_FULL_RELATIONS: &str = "full_relations";
/// Fallback path when a record carries none (`operate.py:2259`).
pub const UNKNOWN_SOURCE: &str = "unknown_source";

#[derive(Debug, Clone)]
pub struct IndexMergeOptions {
    /// `force_llm_summary_on_merge` (`constants.py:30`).
    pub force_llm_summary_on_merge: usize,
    /// `summary_max_tokens` (`constants.py:32`).
    pub summary_max_tokens: usize,
    /// `summary_context_size` (`constants.py:36`).
    pub summary_context_size: usize,
    /// `summary_length_recommended` (`constants.py:34`).
    pub summary_length_recommended: usize,
}

impl Default for IndexMergeOptions {
    fn default() -> Self {
        Self {
            force_llm_summary_on_merge: DEFAULT_FORCE_LLM_SUMMARY_ON_MERGE,
            summary_max_tokens: DEFAULT_SUMMARY_MAX_TOKENS,
            summary_context_size: DEFAULT_SUMMARY_CONTEXT_SIZE,
            summary_length_recommended: DEFAULT_SUMMARY_LENGTH_RECOMMENDED,
        }
    }
}

/// Phase 0: the document's anchor rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeAnchor {
    /// Entity names the document mentions, in first-seen order.
    pub entities: Vec<String>,
    /// Relation keys (sorted pairs joined by `GRAPH_FIELD_SEP`).
    pub relations: Vec<String>,
}

/// Phase 0 of `merge_nodes_and_edges` (`operate.py:3645-3690`): persist the
/// document scope so an interrupted run knows which elements it owns.
pub async fn write_scope_anchor(
    full_entities: &dyn KvStore,
    full_relations: &dyn KvStore,
    doc_id: &str,
    anchor: &ScopeAnchor,
) -> crate::core::traits::Result<()> {
    full_entities
        .upsert(vec![(
            doc_id.to_string(),
            json!({ "entities": anchor.entities }),
        )])
        .await?;
    full_relations
        .upsert(vec![(
            doc_id.to_string(),
            json!({ "relations": anchor.relations }),
        )])
        .await?;
    Ok(())
}

/// Read a document's anchor rows (used by the delete path and by resume).
pub async fn read_scope_anchor(
    full_entities: &dyn KvStore,
    full_relations: &dyn KvStore,
    doc_id: &str,
) -> crate::core::traits::Result<ScopeAnchor> {
    let entities = full_entities.get_by_id(doc_id).await?.and_then(|row| {
        row.get("entities").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
    });
    let relations = full_relations.get_by_id(doc_id).await?.and_then(|row| {
        row.get("relations").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
    });
    Ok(ScopeAnchor {
        entities: entities.unwrap_or_default(),
        relations: relations.unwrap_or_default(),
    })
}

/// The `file_path` list already stored on the node (the reference keeps a
/// `GRAPH_FIELD_SEP`-joined, capped list rather than one path).
fn node_file_paths(node: &Option<Value>) -> Vec<String> {
    node.as_ref()
        .and_then(|node| node.get("file_path").and_then(Value::as_str))
        .map(|value| {
            value
                .split(GRAPH_FIELD_SEP)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The three-tier description summary gate (`operate.py::_handle_entity_relation_summary`
/// :372, thresholds `constants.py:30,32,34,36`).
///
/// Tier 1 — fewer than `force_llm_summary_on_merge` fragments **and** under
/// `summary_max_tokens`: join without an LLM call. Tier 2 — over the token
/// budget: LLM summary. Tier 3 — over `summary_context_size`: split the input,
/// summarise recursively, then join. Returns `(description, llm_called)`.
pub async fn summarize_descriptions(
    name: &str,
    fragments: &[String],
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &IndexMergeOptions,
) -> LlmResult<(String, bool)> {
    if fragments.is_empty() {
        return Ok((String::new(), false));
    }
    let total_tokens: usize = fragments
        .iter()
        .map(|fragment| tokenizer.token_len(fragment))
        .sum();
    if fragments.len() < options.force_llm_summary_on_merge
        && total_tokens <= options.summary_max_tokens
    {
        return Ok((fragments.join(GRAPH_FIELD_SEP), false));
    }

    let mut summaries: Vec<String> = Vec::new();
    if total_tokens > options.summary_context_size {
        // Tier 3: chunk the fragment list, summarise each chunk, recurse on the
        // joined intermediate until it fits the context size.
        let mut batch: Vec<String> = Vec::new();
        let mut batch_tokens = 0usize;
        for fragment in fragments {
            let tokens = tokenizer.token_len(fragment);
            if !batch.is_empty() && batch_tokens + tokens > options.summary_context_size {
                let joined = batch.join(GRAPH_FIELD_SEP);
                summaries.push(summarize_one(name, &joined, llm, options).await?);
                batch.clear();
                batch_tokens = 0;
            }
            batch.push(fragment.clone());
            batch_tokens += tokens;
        }
        if !batch.is_empty() {
            let joined = batch.join(GRAPH_FIELD_SEP);
            summaries.push(summarize_one(name, &joined, llm, options).await?);
        }
        let intermediate = summaries.join(GRAPH_FIELD_SEP);
        if tokenizer.token_len(&intermediate) > options.summary_context_size {
            let boxed: Vec<String> = vec![intermediate];
            return Ok((
                Box::pin(summarize_descriptions(
                    name, &boxed, llm, tokenizer, options,
                ))
                .await?
                .0,
                true,
            ));
        }
        return Ok((intermediate, true));
    }

    // Tier 2: one LLM summary over the joined fragments.
    let joined = fragments.join(GRAPH_FIELD_SEP);
    Ok((summarize_one(name, &joined, llm, options).await?, true))
}

async fn summarize_one(
    name: &str,
    description: &str,
    llm: &CachedLlm,
    options: &IndexMergeOptions,
) -> LlmResult<String> {
    let prompt = crate::core::text::fill_template(
        SUMMARIZE_ENTITY_DESCRIPTIONS,
        &[
            ("description_type", "entity"),
            ("description_name", name),
            ("description_list", description),
            (
                "summary_length",
                &options.summary_length_recommended.to_string(),
            ),
            ("language", "English"),
        ],
    )
    .map_err(|e| LlmError::Decode(format!("summary template: {e}")))?;
    let (summary, _) = llm
        .complete_cached_keyed(
            &prompt,
            None,
            &[],
            &ModelOptions {
                max_tokens: Some(options.summary_max_tokens as u32),
                json_object: false,
            },
            "summary",
        )
        .await?;
    Ok(summary)
}

fn join_with_sep(values: impl IntoIterator<Item = String>) -> String {
    values.into_iter().collect::<Vec<_>>().join(GRAPH_FIELD_SEP)
}

fn dedup_preserving_order(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for value in values {
        if !value.is_empty() && !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

/// `_combine_descriptions_dedup` (`operate.py:2384`): stored fragments keep
/// their order and come first, new fragments append, exact duplicates across
/// both passes drop, and fragments that sanitize to empty drop.
fn combine_descriptions(already: &[String], new: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut combined: Vec<String> = Vec::new();
    for fragment in already
        .iter()
        .chain(new.into_iter().collect::<Vec<_>>().iter())
    {
        let sanitized = sanitize_and_normalize_extracted_text(fragment, false);
        if !sanitized.is_empty() && !combined.contains(&sanitized) {
            combined.push(sanitized);
        }
    }
    combined
}

/// Order the new description fragments the way the reference does: by
/// extraction timestamp, then by descending length
/// (`operate.py:2590-2597`).
fn order_new_descriptions(records: &[crate::core::rag::EntityRecord]) -> Vec<String> {
    let mut unique: Vec<&crate::core::rag::EntityRecord> = Vec::new();
    for record in records {
        if record.description.is_empty() {
            continue;
        }
        if !unique
            .iter()
            .any(|existing| existing.description == record.description)
        {
            unique.push(record);
        }
    }
    unique.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then(b.description.len().cmp(&a.description.len()))
    });
    unique
        .into_iter()
        .map(|record| record.description.clone())
        .collect()
}

/// Phase 1: merge one entity (`_merge_nodes_then_upsert`, operate.py:2429).
///
/// Returns the stored node row (including `entity_name`), which is what the
/// entity vector rows are built from.
pub async fn merge_node(
    entity_name: &str,
    records: &[EntityRecord],
    graph: &dyn GraphStore,
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &IndexMergeOptions,
) -> LlmResult<Value> {
    let already = graph
        .get_node(entity_name)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?;

    let mut already_types: Vec<String> = Vec::new();
    let mut already_sources: Vec<String> = Vec::new();
    let mut already_descriptions: Vec<String> = Vec::new();
    if let Some(node) = &already {
        if let Some(value) = node.get("entity_type").and_then(Value::as_str) {
            already_types.push(value.to_string());
        }
        if let Some(value) = node.get("source_id").and_then(Value::as_str) {
            already_sources.extend(split_string_by_multi_markers(value, &[GRAPH_FIELD_SEP]));
        }
        if let Some(value) = node.get("description").and_then(Value::as_str) {
            already_descriptions.push(value.to_string());
        }
    }

    let entity_type = most_common_type(
        records.iter().map(|record| record.entity_type.clone()),
        already_types.iter().cloned(),
        records,
        &already_types,
    );

    let source_id = join_with_sep(dedup_preserving_order(
        records
            .iter()
            .map(|record| record.source_id.clone())
            .chain(already_sources.iter().cloned()),
    ));

    let description = {
        let ordered_new = order_new_descriptions(records);
        // The reference's no-description fallback (`operate.py:2610`).
        let mut fragments = combine_descriptions(&already_descriptions, ordered_new);
        if fragments.is_empty() {
            fragments.push(format!("Entity {entity_name}"));
        }
        summarize_descriptions(entity_name, &fragments, llm, tokenizer, options)
            .await?
            .0
    };

    // `max_file_paths` defaults to 75 (`constants.py:84`); we do not cap the
    // list yet — a document set large enough to matter would already be past
    // the source-id cap, and the placeholder token is a display concern.
    let file_path = {
        let mut paths: Vec<String> = Vec::new();
        let push_path = |path: &str, paths: &mut Vec<String>| {
            if !path.is_empty() && !paths.iter().any(|existing| existing == path) {
                paths.push(path.to_string());
            }
        };
        for path in node_file_paths(&already) {
            push_path(&path, &mut paths);
        }
        for record in records {
            push_path(&record.file_path, &mut paths);
        }
        if paths.is_empty() {
            UNKNOWN_SOURCE.to_string()
        } else {
            paths.join(GRAPH_FIELD_SEP)
        }
    };
    let created_at = records
        .iter()
        .map(|record| record.timestamp)
        .find(|timestamp| *timestamp > 0)
        .or_else(|| {
            already
                .as_ref()
                .and_then(|node| node.get("created_at").and_then(Value::as_i64))
        })
        .unwrap_or(0);

    let mut node_data = json!({
        "entity_type": entity_type,
        "description": description,
        "source_id": source_id,
        "file_path": file_path,
        "created_at": created_at,
        "truncate": "",
    });
    graph
        .upsert_node(entity_name, node_data.clone())
        .await
        .map_err(|e| LlmError::Transport(format!("graph write: {e}")))?;
    node_data["entity_name"] = json!(entity_name);
    Ok(node_data)
}

/// Counter.most_common semantics: highest count wins, ties keep the first
/// appearance (the reference sorts stably by count).
fn most_common_type(
    new_types: impl Iterator<Item = String>,
    old_types: impl Iterator<Item = String>,
    records: &[EntityRecord],
    already_types: &[String],
) -> String {
    let mut candidates: Vec<String> = Vec::new();
    for value in new_types.chain(old_types) {
        if !candidates.contains(&value) {
            candidates.push(value);
        }
    }
    let mut best = String::new();
    let mut best_count = 0usize;
    for candidate in &candidates {
        let count = records
            .iter()
            .filter(|record| record.entity_type == *candidate)
            .count()
            + already_types
                .iter()
                .filter(|value| *value == candidate)
                .count();
        if count > best_count {
            best_count = count;
            best = candidate.clone();
        }
    }
    best
}

/// Phase 2: merge one relation (`_merge_edges_then_upsert`, operate.py:2782).
///
/// Returns the graph edge payload plus the tracking key, so the caller can
/// update `relation_chunks` without re-deriving the pair.
pub async fn merge_edge(
    src_id: &str,
    tgt_id: &str,
    records: &[RelationRecord],
    graph: &dyn GraphStore,
    llm: &CachedLlm,
    tokenizer: &Tokenizer,
    options: &IndexMergeOptions,
) -> LlmResult<Value> {
    let mut already_weights = 0.0f64;
    let mut already_sources: Vec<String> = Vec::new();
    let mut already_descriptions: Vec<String> = Vec::new();
    let mut already_keywords: Vec<String> = Vec::new();
    if let Some(edge) = graph
        .get_edge(src_id, tgt_id)
        .await
        .map_err(|e| LlmError::Transport(format!("graph read: {e}")))?
    {
        already_weights = edge.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
        if let Some(value) = edge.get("source_id").and_then(Value::as_str) {
            already_sources.extend(split_string_by_multi_markers(value, &[GRAPH_FIELD_SEP]));
        }
        if let Some(value) = edge.get("description").and_then(Value::as_str) {
            already_descriptions.push(value.to_string());
        }
        if let Some(value) = edge.get("keywords").and_then(Value::as_str) {
            already_keywords.extend(split_string_by_multi_markers(value, &[GRAPH_FIELD_SEP]));
        }
    }

    let source_id = join_with_sep(dedup_preserving_order(
        records
            .iter()
            .map(|record| record.source_id.clone())
            .chain(already_sources.iter().cloned()),
    ));
    let already_source_set: Vec<String> = already_sources.clone();
    let weight = records
        .iter()
        .filter(|record| !already_source_set.contains(&record.source_id))
        .map(|record| record.weight)
        .sum::<f64>()
        + already_weights;
    // Floor at the number of distinct real evidence sources so a recovered or
    // legacy row never reports less evidence than it stores.
    let evidence_count = dedup_preserving_order(
        records
            .iter()
            .map(|record| record.source_id.clone())
            .chain(already_sources.iter().cloned()),
    )
    .len();
    let weight = weight.max(evidence_count as f64);

    let description = {
        let ordered_new: Vec<String> = {
            let mut unique: Vec<&RelationRecord> = Vec::new();
            for record in records {
                if record.description.is_empty() {
                    continue;
                }
                if !unique
                    .iter()
                    .any(|existing| existing.description == record.description)
                {
                    unique.push(record);
                }
            }
            unique.sort_by(|a, b| {
                a.timestamp
                    .cmp(&b.timestamp)
                    .then(b.description.len().cmp(&a.description.len()))
            });
            unique
                .into_iter()
                .map(|record| record.description.clone())
                .collect()
        };
        let mut fragments = combine_descriptions(&already_descriptions, ordered_new);
        if fragments.is_empty() {
            fragments.push(format!("Relation {src_id}~{tgt_id}"));
        }
        let name = py_repr_tuple2(src_id, tgt_id);
        summarize_descriptions(&name, &fragments, llm, tokenizer, options)
            .await?
            .0
    };
    // `operate.py:2170-2178`: split comma-separated tokens, dedup, sort, join
    // with ", " — and keep the stored keywords verbatim when nothing is new.
    let keywords = {
        let mut all: BTreeSet<String> = BTreeSet::new();
        for existing in &already_keywords {
            for token in existing.split(',') {
                let token = token.trim();
                if !token.is_empty() {
                    all.insert(token.to_string());
                }
            }
        }
        for record in records {
            for token in record.keywords.split(',') {
                let token = token.trim();
                if !token.is_empty() {
                    all.insert(token.to_string());
                }
            }
        }
        if all.is_empty() {
            already_keywords.join(GRAPH_FIELD_SEP)
        } else {
            all.into_iter().collect::<Vec<_>>().join(", ")
        }
    };

    let file_path = records
        .iter()
        .map(|record| record.file_path.clone())
        .find(|path| !path.is_empty())
        .unwrap_or_else(|| UNKNOWN_SOURCE.to_string());
    let created_at = records
        .iter()
        .map(|record| record.timestamp)
        .find(|timestamp| *timestamp > 0)
        .unwrap_or(0);

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
                        "description": "",
                        "entity_type": "UNKNOWN",
                        "file_path": file_path,
                        "created_at": created_at,
                        "truncate": "",
                    }),
                )
                .await
                .map_err(|e| LlmError::Transport(format!("graph write: {e}")))?;
        }
    }

    let edge_data = json!({
        "weight": weight,
        "description": description,
        "keywords": keywords,
        "source_id": source_id,
        "file_path": file_path,
        "created_at": created_at,
        "truncate": "",
    });
    graph
        .upsert_edge(src_id, tgt_id, edge_data.clone())
        .await
        .map_err(|e| LlmError::Transport(format!("graph write: {e}")))?;
    Ok(edge_data)
}

/// Entity vector rows — `ent-{md5(name)}` ids and `name + description` content
/// (`operate.py:2750-2756`).
pub fn entity_vector_rows(merged_nodes: &[Value]) -> Vec<VectorRow> {
    merged_nodes
        .iter()
        .map(|node| {
            let name = node["entity_name"].as_str().unwrap_or_default();
            VectorRow {
                id: compute_mdhash_id(name, "ent-"),
                content: format!("{name}{}", node["description"].as_str().unwrap_or_default()),
                meta: json!({
                    "entity_name": name,
                    "entity_type": node.get("entity_type").cloned().unwrap_or(json!("UNKNOWN")),
                    "source_id": node.get("source_id").cloned().unwrap_or(json!("")),
                    "file_path": node.get("file_path").cloned().unwrap_or(json!(UNKNOWN_SOURCE)),
                    "created_at": node.get("created_at").cloned().unwrap_or(json!(0)),
                }),
            }
        })
        .collect()
}

/// Relation vector rows — `rel-{md5(sorted_pair)}`, both directions written,
/// content `keywords \t src \n tgt \n description` (`operate.py:3388-3400`).
pub fn relation_vector_rows(
    src_id: &str,
    tgt_id: &str,
    edge: &Value,
    source_id: &str,
) -> Vec<VectorRow> {
    let keywords = edge.get("keywords").and_then(Value::as_str).unwrap_or("");
    let description = edge
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = format!("{keywords}\t{src_id}\n{tgt_id}\n{description}");
    let meta = json!({
        "src_id": src_id,
        "tgt_id": tgt_id,
        "source_id": source_id,
        "keywords": keywords,
        "description": description,
        "weight": edge.get("weight").cloned().unwrap_or(json!(1.0)),
        "file_path": edge.get("file_path").cloned().unwrap_or(json!(UNKNOWN_SOURCE)),
        "created_at": edge.get("created_at").cloned().unwrap_or(json!(0)),
    });
    let forward = compute_mdhash_id(&format!("{src_id}{tgt_id}"), "rel-");
    let reverse = compute_mdhash_id(&format!("{tgt_id}{src_id}"), "rel-");
    vec![
        VectorRow {
            id: forward,
            content: content.clone(),
            meta: meta.clone(),
        },
        VectorRow {
            id: reverse,
            content,
            meta,
        },
    ]
}

/// Upsert entity vector rows in batches, queueing failures for repair.
pub async fn upsert_entity_vectors(
    entities_vdb: &dyn VectorStore,
    rows: &[VectorRow],
    batch_num: usize,
    repair: &crate::store::repair::RepairQueue,
) -> crate::core::traits::Result<()> {
    for batch in rows.chunks(batch_num.max(1)) {
        if let Err(error) = entities_vdb.upsert(batch.to_vec()).await {
            for row in batch {
                repair
                    .record(
                        crate::store::repair::RepairTarget::EntityVector,
                        &row.id,
                        json!([row.clone()]),
                    )
                    .await?;
            }
            tracing::warn!(%error, rows = batch.len(), "entity vector write deferred to repair");
        }
    }
    Ok(())
}

/// Upsert relation vector rows in batches, queueing failures for repair.
pub async fn upsert_relation_vectors(
    relationships_vdb: &dyn VectorStore,
    rows: &[VectorRow],
    batch_num: usize,
    repair: &crate::store::repair::RepairQueue,
) -> crate::core::traits::Result<()> {
    for batch in rows.chunks(batch_num.max(1)) {
        if let Err(error) = relationships_vdb.upsert(batch.to_vec()).await {
            for row in batch {
                repair
                    .record(
                        crate::store::repair::RepairTarget::RelationVector,
                        &row.id,
                        json!([row.clone()]),
                    )
                    .await?;
            }
            tracing::warn!(%error, rows = batch.len(), "relation vector write deferred to repair");
        }
    }
    Ok(())
}

/// The two tracking-row maps one document's extraction produces.
pub type TrackingRows = (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>);

/// Tracking rows for one document's extraction: entity name → chunk ids and
/// relation key → chunk ids.
pub fn tracking_rows(
    records: &crate::graph::extract::ExtractedRecords,
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let mut entities: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, rows) in &records.nodes {
        let ids: Vec<String> = rows.iter().map(|record| record.source_id.clone()).collect();
        entities.entry(name.clone()).or_default().extend(ids);
    }
    let mut relations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ((src, tgt), rows) in &records.edges {
        let key = make_relation_chunk_key(src, tgt);
        let ids: Vec<String> = rows.iter().map(|record| record.source_id.clone()).collect();
        relations.entry(key).or_default().extend(ids);
    }
    (entities, relations)
}

/// Merge tracking rows through the truth layer, queueing failures for repair.
pub async fn write_tracking_rows(
    tracking: &TrackingStores,
    rows: &TrackingRows,
    source_ids_limit: usize,
    limit_method: &str,
    repair: &crate::store::repair::RepairQueue,
) -> crate::core::traits::Result<()> {
    let (entities, relations) = rows;
    let merged_entities = merge_tracking_rows_dry(
        tracking.entity_chunks.as_ref(),
        entities,
        source_ids_limit,
        limit_method,
    )
    .await?;
    let merged_relations = merge_tracking_rows_dry(
        tracking.relation_chunks.as_ref(),
        relations,
        source_ids_limit,
        limit_method,
    )
    .await?;
    if let Err(error) = tracking.entity_chunks.upsert(merged_entities.clone()).await {
        for (key, value) in &merged_entities {
            repair
                .record(
                    crate::store::repair::RepairTarget::EntityChunk,
                    key,
                    value.clone(),
                )
                .await?;
        }
        tracing::warn!(%error, "entity_chunks write deferred to repair");
    }
    if let Err(error) = tracking
        .relation_chunks
        .upsert(merged_relations.clone())
        .await
    {
        for (key, value) in &merged_relations {
            repair
                .record(
                    crate::store::repair::RepairTarget::RelationChunk,
                    key,
                    value.clone(),
                )
                .await?;
        }
        tracing::warn!(%error, "relation_chunks write deferred to repair");
    }
    Ok(())
}

async fn merge_tracking_rows_dry(
    store: &dyn KvStore,
    rows: &BTreeMap<String, Vec<String>>,
    source_ids_limit: usize,
    limit_method: &str,
) -> crate::core::traits::Result<Vec<(String, Value)>> {
    let mut upserts = Vec::new();
    for (key, new_ids) in rows {
        let existing = store
            .get_by_id(key)
            .await?
            .and_then(|row| {
                row.get("chunks").and_then(Value::as_array).map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<String>>()
                })
            })
            .unwrap_or_default();
        let merged = crate::index::tracking::merge_source_ids(
            &existing,
            new_ids,
            source_ids_limit,
            limit_method,
        );
        if merged != existing {
            upserts.push((key.clone(), json!({ "chunks": merged })));
        }
    }
    Ok(upserts)
}
