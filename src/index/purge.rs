//! Cascade delete — LightRAG's `adelete_by_doc_id` family.
//!
//! Deleting a document must not delete knowledge other documents still own.
//! The reference's algorithm (`lightrag.py::adelete_by_doc_id` :6727,
//! `_purge_kg_contributions` :6015):
//!
//! 1. the document's chunk ids come from `text_chunks`;
//! 2. candidates = union of `entity_chunks`/`relation_chunks` tracking entries
//!    and the graph rows' own `source_id` lists;
//! 3. for each candidate, intersect the deleted chunks with its full source
//!    set — **no remaining source: delete outright** (row, tracking row, and
//!    for relations the reverse vector id), otherwise **rebuild** from the
//!    surviving chunks' cached extraction (`operate.py::
//!    rebuild_knowledge_from_chunks` :1102);
//! 4. chunk rows and their vector rows go last, so no graph object can point
//!    at a deleted chunk;
//! 5. `llm_cache_list` references recorded on the chunk rows are dropped with
//!    the chunks.
//!
//! Deviation detail: fragment attribution needs the chunk rows'
//! `llm_cache_list` written at insert time (port of
//! `update_chunk_cache_list`, `utils.py:5136`). A surviving chunk whose row
//! carries no cache references (its insert ran with the extraction cache off)
//! is unattributable — then the merge description is kept verbatim (a safe
//! superset, logged). `source_id` and `file_path` always rebuild from the
//! chunk truth rows, which every path can attribute.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::core::rag::Chunk;
use crate::core::text::Tokenizer;
use crate::core::text::{compute_mdhash_id, split_string_by_multi_markers, GRAPH_FIELD_SEP};
use crate::core::traits::{GraphStore, KvStore, StoreError, VectorRow, VectorStore};
use crate::graph::extract::{parse_extraction_result, ExtractedRecords};
use crate::index::merge::IndexMergeOptions;
use crate::index::tracking::{make_relation_chunk_key, parse_relation_chunk_key, TrackingStores};
use crate::llm::cache::CachedLlm;
use crate::llm::LlmResult;
use tracing::warn;

/// What one purge step did to a graph element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementAction {
    /// No remaining source: the row and its tracking row were deleted.
    Deleted,
    /// Surviving sources remain: the row was rebuilt from them.
    Rebuilt,
}

#[derive(Debug, Clone, Default)]
pub struct PurgeReport {
    pub deleted_entities: Vec<String>,
    pub rebuilt_entities: Vec<String>,
    pub deleted_relations: Vec<String>,
    pub rebuilt_relations: Vec<String>,
    pub deleted_chunks: usize,
}

/// Everything the delete path touches.
pub struct PurgeStores<'a> {
    pub graph: &'a dyn GraphStore,
    pub entities_vdb: &'a dyn VectorStore,
    pub relationships_vdb: Option<&'a dyn VectorStore>,
    pub text_chunks: &'a dyn KvStore,
    pub full_docs: &'a dyn KvStore,
    pub full_entities: &'a dyn KvStore,
    pub full_relations: &'a dyn KvStore,
    pub llm_cache: &'a dyn KvStore,
    pub tracking: &'a TrackingStores,
    pub llm: &'a CachedLlm,
    pub tokenizer: &'a Tokenizer,
    pub options: &'a IndexMergeOptions,
}

/// `adelete_by_doc_id` (`lightrag.py:6727`).
pub async fn delete_document(
    doc_id: &str,
    stores: &PurgeStores<'_>,
) -> crate::core::traits::Result<PurgeReport> {
    let mut report = PurgeReport::default();
    let Some(document) = stores.full_docs.get_by_id(doc_id).await? else {
        return Ok(report);
    };
    let _ = document;

    // 1. The document's chunk ids — the anchor of the whole cascade — plus
    //    the cache keys those chunks recorded on their own rows, collected
    //    while the rows still exist (the forward path, `adelete_by_doc_id`
    //    :7078-7103).
    let mut chunk_ids: Vec<String> = Vec::new();
    let mut cache_refs: BTreeSet<String> = BTreeSet::new();
    for key in stores.text_chunks.all_keys().await? {
        let Some(row) = stores.text_chunks.get_by_id(&key).await? else {
            continue;
        };
        if row.get("full_doc_id").and_then(Value::as_str) == Some(doc_id) {
            chunk_ids.push(key);
            if let Some(list) = row.get("llm_cache_list").and_then(Value::as_array) {
                for item in list {
                    if let Some(cache_key) = item.as_str() {
                        cache_refs.insert(cache_key.to_string());
                    }
                }
            }
        }
    }
    if chunk_ids.is_empty() {
        // Nothing derived: drop the document row and the anchors only.
        stores.full_docs.remove(&[doc_id.to_string()]).await?;
        stores.full_entities.remove(&[doc_id.to_string()]).await?;
        stores.full_relations.remove(&[doc_id.to_string()]).await?;
        return Ok(report);
    }
    let deleted: BTreeSet<String> = chunk_ids.iter().cloned().collect();

    // 2. Candidates from the Phase-0 anchors plus every tracking row whose id
    //    list intersects the deleted set.
    let anchor_entities = anchor_rows(stores.full_entities, doc_id).await?;
    let anchor_relations = anchor_rows(stores.full_relations, doc_id).await?;
    let entity_candidates = candidate_entities(stores, &deleted, &anchor_entities).await?;
    let relation_candidates = candidate_relations(stores, &deleted, &anchor_relations).await?;

    let mut attributions: BTreeMap<String, ChunkAttribution> = BTreeMap::new();

    // 3. Classify and act on each candidate.
    for name in &entity_candidates {
        let remaining = remaining_sources_entity(stores, name, &deleted).await?;
        if remaining.is_empty() {
            stores.graph.remove_node(name).await?;
            stores
                .tracking
                .entity_chunks
                .remove(std::slice::from_ref(name))
                .await?;
            stores
                .entities_vdb
                .remove(&[compute_mdhash_id(name, "ent-")])
                .await?;
            report.deleted_entities.push(name.clone());
        } else {
            ensure_attributions(stores, &mut attributions, &remaining).await?;
            rebuild_entity(stores, &attributions, name, &remaining)
                .await
                .map_err(|e| StoreError::Backend(format!("entity rebuild: {e}")))?;
            report.rebuilt_entities.push(name.clone());
        }
    }
    for key in &relation_candidates {
        let Some((src, tgt)) = parse_relation_chunk_key(key) else {
            continue;
        };
        let remaining = remaining_sources_relation(stores, &src, &tgt, &deleted).await?;
        if remaining.is_empty() {
            stores.graph.remove_edge(&src, &tgt).await?;
            stores
                .tracking
                .relation_chunks
                .remove(std::slice::from_ref(key))
                .await?;
            if let Some(vdb) = stores.relationships_vdb {
                let forward = compute_mdhash_id(&format!("{src}{tgt}"), "rel-");
                let reverse = compute_mdhash_id(&format!("{tgt}{src}"), "rel-");
                vdb.remove(&[forward, reverse]).await?;
            }
            report.deleted_relations.push(key.clone());
        } else {
            ensure_attributions(stores, &mut attributions, &remaining).await?;
            rebuild_relation(stores, &attributions, &src, &tgt, &remaining)
                .await
                .map_err(|e| StoreError::Backend(format!("relation rebuild: {e}")))?;
            report.rebuilt_relations.push(key.clone());
        }
    }

    // 4. Chunk rows go last: no graph object may point at a deleted chunk. The
    // chunk vector rows live in `chunks_vdb`, which the pipeline owns, so the
    // pipeline deletes them alongside this call.
    stores.text_chunks.remove(&chunk_ids).await?;
    report.deleted_chunks = chunk_ids.len();

    // 5. The cache rows the deleted chunks referenced die with them: the
    // forward `llm_cache_list` references plus the legacy reverse scan.
    drop_cached_references(stores.llm_cache, &deleted, &cache_refs).await?;

    // 6. Anchors and the document row.
    stores.full_docs.remove(&[doc_id.to_string()]).await?;
    stores.full_entities.remove(&[doc_id.to_string()]).await?;
    stores.full_relations.remove(&[doc_id.to_string()]).await?;
    Ok(report)
}

async fn anchor_rows(
    store: &dyn KvStore,
    doc_id: &str,
) -> crate::core::traits::Result<Vec<String>> {
    Ok(store
        .get_by_id(doc_id)
        .await?
        .and_then(|row| {
            row.as_object().and_then(|object| {
                object
                    .get("entities")
                    .or_else(|| object.get("relations"))
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
            })
        })
        .unwrap_or_default())
}

async fn candidate_entities(
    stores: &PurgeStores<'_>,
    deleted: &BTreeSet<String>,
    anchor: &[String],
) -> crate::core::traits::Result<Vec<String>> {
    let mut candidates: BTreeSet<String> = anchor.iter().cloned().collect();
    for key in stores.tracking.entity_chunks.all_keys().await? {
        let ids = tracking_ids(stores.tracking.entity_chunks.as_ref(), &key).await?;
        if ids.iter().any(|id| deleted.contains(id)) {
            candidates.insert(key);
        }
    }
    // Graph rows that mention one of the deleted chunks are candidates even if
    // their tracking row is missing (legacy or partially-repaired store).
    let snapshot = stores.graph.snapshot().await?;
    for (name, props) in &snapshot.nodes {
        if source_ids(props).iter().any(|id| deleted.contains(id)) {
            candidates.insert(name.clone());
        }
    }
    Ok(candidates.into_iter().collect())
}

async fn candidate_relations(
    stores: &PurgeStores<'_>,
    deleted: &BTreeSet<String>,
    anchor: &[String],
) -> crate::core::traits::Result<Vec<String>> {
    let mut candidates: BTreeSet<String> = anchor.iter().cloned().collect();
    for key in stores.tracking.relation_chunks.all_keys().await? {
        let ids = tracking_ids(stores.tracking.relation_chunks.as_ref(), &key).await?;
        if ids.iter().any(|id| deleted.contains(id)) {
            candidates.insert(key);
        }
    }
    let snapshot = stores.graph.snapshot().await?;
    for (src, tgt, props) in &snapshot.edges {
        if source_ids(props).iter().any(|id| deleted.contains(id)) {
            candidates.insert(make_relation_chunk_key(src, tgt));
        }
    }
    Ok(candidates.into_iter().collect())
}

async fn tracking_ids(store: &dyn KvStore, key: &str) -> crate::core::traits::Result<Vec<String>> {
    Ok(store
        .get_by_id(key)
        .await?
        .and_then(|row| {
            row.get("chunks").and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
        })
        .unwrap_or_default())
}

fn source_ids(props: &Value) -> Vec<String> {
    props
        .get("source_id")
        .and_then(Value::as_str)
        .map(|value| split_string_by_multi_markers(value, &[GRAPH_FIELD_SEP]))
        .unwrap_or_default()
}

/// Remaining sources for an entity: its tracking row and graph row minus the
/// deleted chunks.
async fn remaining_sources_entity(
    stores: &PurgeStores<'_>,
    name: &str,
    deleted: &BTreeSet<String>,
) -> crate::core::traits::Result<Vec<String>> {
    let mut sources: BTreeSet<String> = tracking_ids(stores.tracking.entity_chunks.as_ref(), name)
        .await?
        .into_iter()
        .collect();
    if let Some(node) = stores.graph.get_node(name).await? {
        sources.extend(source_ids(&node));
    }
    Ok(sources
        .into_iter()
        .filter(|id| !deleted.contains(id))
        .collect())
}

async fn remaining_sources_relation(
    stores: &PurgeStores<'_>,
    src: &str,
    tgt: &str,
    deleted: &BTreeSet<String>,
) -> crate::core::traits::Result<Vec<String>> {
    let key = make_relation_chunk_key(src, tgt);
    let mut sources: BTreeSet<String> =
        tracking_ids(stores.tracking.relation_chunks.as_ref(), &key)
            .await?
            .into_iter()
            .collect();
    if let Some(edge) = stores.graph.get_edge(src, tgt).await? {
        sources.extend(source_ids(&edge));
    }
    Ok(sources
        .into_iter()
        .filter(|id| !deleted.contains(id))
        .collect())
}

/// What a surviving chunk can contribute to a rebuild: the cached extraction
/// rows its `llm_cache_list` points at (re-parsed exactly like the reference
/// `rebuild_knowledge_from_chunks` does, `operate.py:1102`) and its truth-row
/// `file_path`. `extraction` is `None` when the row predates the forward
/// cache bookkeeping — unattributable.
struct ChunkAttribution {
    extraction: Option<ExtractedRecords>,
    file_path: String,
}

/// Fill the memo for every surviving chunk (idempotent per delete run).
async fn ensure_attributions(
    stores: &PurgeStores<'_>,
    memo: &mut BTreeMap<String, ChunkAttribution>,
    remaining: &[String],
) -> crate::core::traits::Result<()> {
    for chunk in remaining {
        if memo.contains_key(chunk) {
            continue;
        }
        let row = stores.text_chunks.get_by_id(chunk).await?;
        let file_path = row
            .as_ref()
            .and_then(|row| row.get("file_path"))
            .and_then(Value::as_str)
            .unwrap_or("unknown_source")
            .to_string();
        let extraction = match row
            .as_ref()
            .and_then(|row| row.get("llm_cache_list"))
            .and_then(Value::as_array)
        {
            Some(cache_keys) => {
                let mut aggregated = ExtractedRecords::default();
                for item in cache_keys {
                    let Some(cache_key) = item.as_str() else {
                        continue;
                    };
                    let Some(cached) = stores.llm_cache.get_by_id(cache_key).await? else {
                        continue;
                    };
                    let Some(text) = cached.get("return").and_then(Value::as_str) else {
                        continue;
                    };
                    let parsed = parse_extraction_result(text, chunk, &file_path);
                    aggregated.nodes.extend(parsed.nodes);
                    aggregated.edges.extend(parsed.edges);
                }
                Some(aggregated)
            }
            None => None,
        };
        memo.insert(
            chunk.clone(),
            ChunkAttribution {
                extraction,
                file_path,
            },
        );
    }
    Ok(())
}

/// `Some((description, file_path))` when every surviving chunk attributes;
/// `None` when any chunk lacks cache bookkeeping (caller keeps the stored
/// description as the safe superset). Fragments keep chunk order, dedup on
/// exact text — the same first-encounter order the insert-time merge used.
fn rebuilt_fragments(
    attributions: &BTreeMap<String, ChunkAttribution>,
    remaining: &[String],
    mut select: impl FnMut(&ExtractedRecords) -> Vec<String>,
) -> Option<(String, String)> {
    let mut fragments: Vec<String> = Vec::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for chunk in remaining {
        let attribution = attributions.get(chunk)?;
        paths.insert(attribution.file_path.clone());
        let extraction = attribution.extraction.as_ref()?;
        for fragment in select(extraction) {
            for piece in fragment
                .split(GRAPH_FIELD_SEP)
                .filter(|piece| !piece.trim().is_empty())
            {
                if !fragments.iter().any(|kept| kept == piece) {
                    fragments.push(piece.to_string());
                }
            }
        }
    }
    Some((
        fragments.join(GRAPH_FIELD_SEP),
        surviving_file_paths(attributions, remaining),
    ))
}

fn surviving_file_paths(
    attributions: &BTreeMap<String, ChunkAttribution>,
    remaining: &[String],
) -> String {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for chunk in remaining {
        if let Some(attribution) = attributions.get(chunk) {
            paths.insert(attribution.file_path.clone());
        }
    }
    paths
        .into_iter()
        .collect::<Vec<String>>()
        .join(GRAPH_FIELD_SEP)
}

/// Patch a node attribute set in place (`upsert_node` is last-write-wins per
/// key, mirroring networkx `add_node`).
fn apply_patch(target: &mut Value, patch: &Value) {
    if let (Some(base), Some(changes)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in changes {
            base.insert(key.clone(), value.clone());
        }
    }
}

/// Rebuild an entity from its surviving chunks' cached extraction, dropping
/// the description fragments the deleted chunks contributed.
async fn rebuild_entity(
    stores: &PurgeStores<'_>,
    attributions: &BTreeMap<String, ChunkAttribution>,
    name: &str,
    remaining: &[String],
) -> LlmResult<()> {
    let Some(node) = stores
        .graph
        .get_node(name)
        .await
        .map_err(|e| crate::llm::LlmError::Transport(format!("graph read: {e}")))?
    else {
        return Ok(());
    };
    let mut patch = json!({
        "entity_name": name,
        "source_id": remaining.join(GRAPH_FIELD_SEP),
    });
    match rebuilt_fragments(attributions, remaining, |extraction| {
        extraction
            .nodes
            .iter()
            .filter(move |(entity, _)| entity == name)
            .flat_map(|(_, records)| records.iter().map(|record| record.description.clone()))
            .collect()
    }) {
        Some((description, file_path)) => {
            patch["description"] = json!(description);
            patch["file_path"] = json!(file_path);
        }
        None => {
            warn!(
                entity = name,
                "kept the stored description: a surviving chunk carries no \
                                  llm_cache_list to attribute fragments"
            );
            patch["file_path"] = json!(surviving_file_paths(attributions, remaining));
        }
    }
    stores
        .graph
        .upsert_node(name, patch.clone())
        .await
        .map_err(|e| crate::llm::LlmError::Transport(format!("graph write: {e}")))?;
    let mut merged = node;
    apply_patch(&mut merged, &patch);
    let rows = crate::index::merge::entity_vector_rows(std::slice::from_ref(&merged));
    stores
        .entities_vdb
        .upsert(rows)
        .await
        .map_err(|e| crate::llm::LlmError::Transport(format!("vector write: {e}")))?;
    Ok(())
}

async fn rebuild_relation(
    stores: &PurgeStores<'_>,
    attributions: &BTreeMap<String, ChunkAttribution>,
    src: &str,
    tgt: &str,
    remaining: &[String],
) -> LlmResult<()> {
    let Some(edge) = stores
        .graph
        .get_edge(src, tgt)
        .await
        .map_err(|e| crate::llm::LlmError::Transport(format!("graph read: {e}")))?
    else {
        return Ok(());
    };
    let pair = if src <= tgt {
        (src.to_string(), tgt.to_string())
    } else {
        (tgt.to_string(), src.to_string())
    };
    let mut patch = json!({
        "source_id": remaining.join(GRAPH_FIELD_SEP),
    });
    match rebuilt_fragments(attributions, remaining, |extraction| {
        extraction
            .edges
            .iter()
            .filter(|(edge_pair, _)| *edge_pair == pair)
            .flat_map(|(_, records)| records.iter().map(|record| record.description.clone()))
            .collect()
    }) {
        Some((description, file_path)) => {
            patch["description"] = json!(description);
            patch["file_path"] = json!(file_path);
        }
        None => {
            warn!(relation = %make_relation_chunk_key(src, tgt), "kept the stored description: a \
                                                                  surviving chunk carries no \
                                                                  llm_cache_list to attribute \
                                                                  fragments");
            patch["file_path"] = json!(surviving_file_paths(attributions, remaining));
        }
    }
    stores
        .graph
        .upsert_edge(src, tgt, patch.clone())
        .await
        .map_err(|e| crate::llm::LlmError::Transport(format!("graph write: {e}")))?;
    if let Some(vdb) = stores.relationships_vdb {
        let mut merged = edge;
        apply_patch(&mut merged, &patch);
        let rows = crate::index::merge::relation_vector_rows(
            src,
            tgt,
            &merged,
            &remaining.join(GRAPH_FIELD_SEP),
        );
        vdb.upsert(rows)
            .await
            .map_err(|e| crate::llm::LlmError::Transport(format!("vector write: {e}")))?;
    }
    Ok(())
}

/// Drop the LLM cache rows the deleted chunks referenced.
///
/// Two passes: the forward one is the reference's — cache keys recorded on the
/// chunk rows' `llm_cache_list` by `use_llm_func_with_cache`
/// (`utils.py:5617-5643`), read back by `adelete_by_doc_id` (:7078-7103); the
/// legacy reverse scan keeps rows that embed `chunk_ids` themselves, which
/// no cache row written today does.
async fn drop_cached_references(
    llm_cache: &dyn KvStore,
    deleted: &BTreeSet<String>,
    chunk_cache_refs: &BTreeSet<String>,
) -> crate::core::traits::Result<()> {
    let mut keys: BTreeSet<String> = chunk_cache_refs.clone();
    for key in llm_cache.all_keys().await? {
        let ids = llm_cache
            .get_by_id(&key)
            .await?
            .and_then(|row| {
                row.get("chunk_ids").and_then(Value::as_array).map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<String>>()
                })
            })
            .unwrap_or_default();
        if ids.iter().any(|id| deleted.contains(id)) {
            keys.insert(key);
        }
    }
    if !keys.is_empty() {
        let removed: Vec<String> = keys.into_iter().collect();
        llm_cache.remove(&removed).await?;
    }
    Ok(())
}

/// Vector-row helper used by the pipeline's naive-RAG delete path.
pub fn chunk_vector_rows(chunks: &[(String, Chunk)]) -> Vec<VectorRow> {
    chunks
        .iter()
        .map(|(key, chunk)| VectorRow {
            id: key.clone(),
            content: chunk.content.clone(),
            meta: json!({
                "tokens": chunk.tokens,
                "chunk_order_index": chunk.chunk_order_index,
                "full_doc_id": chunk.full_doc_id,
                "file_path": chunk.file_path,
            }),
        })
        .collect()
}

/// Tracking-row bookkeeping shared with the merge module: the entity and
/// relation keys one document contributes.
pub fn document_scope_keys(
    records: &crate::graph::extract::ExtractedRecords,
) -> (Vec<String>, Vec<String>) {
    let entities: Vec<String> = records.nodes.iter().map(|(name, _)| name.clone()).collect();
    let relations: Vec<String> = records
        .edges
        .iter()
        .map(|((src, tgt), _)| make_relation_chunk_key(src, tgt))
        .collect();
    (entities, relations)
}

/// Empty-key helper kept beside the classification logic: an element with no
/// source at all is dangling and must be dropped even outside a purge.
pub async fn dangling_entities(
    graph: &dyn GraphStore,
    tracking: &TrackingStores,
) -> crate::core::traits::Result<Vec<String>> {
    let snapshot = graph.snapshot().await?;
    let mut dangling = Vec::new();
    for (name, props) in &snapshot.nodes {
        if source_ids(props).is_empty() {
            let tracked = tracking_ids(tracking.entity_chunks.as_ref(), name).await?;
            if tracked.is_empty() {
                dangling.push(name.clone());
            }
        }
    }
    Ok(dangling)
}

/// BTreeMap re-export used by tests to build fixtures.
pub type TrackingMap = BTreeMap<String, Vec<String>>;
