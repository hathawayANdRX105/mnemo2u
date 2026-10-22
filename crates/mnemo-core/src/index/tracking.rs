//! Chunk-tracking rows — the `entity_chunks` / `relation_chunks` stores that
//! make incremental writes and selective deletes possible.
//!
//! Each row maps one graph element (entity name, or a sorted entity pair) to
//! the chunk ids that produced it. Writes merge and cap the list; deletes read
//! them to decide which elements are affected by a chunk removal
//! (`utils.py::make_relation_chunk_key` :7383, `merge_source_ids` :7183,
//! `apply_source_ids_limit` :7244).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::core::text::GRAPH_FIELD_SEP;
use crate::core::traits::KvStore;
use crate::store::repair::RepairQueue;

/// KV namespace for entity→chunk rows.
pub const NS_ENTITY_CHUNKS: &str = "entity_chunks";
/// KV namespace for relation→chunk rows.
pub const NS_RELATION_CHUNKS: &str = "relation_chunks";

/// `SOURCE_IDS_LIMIT_METHOD_KEEP` (`constants.py:76`): keep the oldest ids.
pub const LIMIT_KEEP: &str = "KEEP";
/// `SOURCE_IDS_LIMIT_METHOD_FIFO` (`constants.py:77`): keep the newest ids.
pub const LIMIT_FIFO: &str = "FIFO";
/// `DEFAULT_SOURCE_IDS_LIMIT_METHOD` (`constants.py:78`).
pub const DEFAULT_SOURCE_IDS_LIMIT_METHOD: &str = LIMIT_KEEP;

/// `apply_source_ids_limit` (`utils.py:7244-7284`).
pub fn apply_source_ids_limit(ids: &[String], limit: usize, method: &str) -> Vec<String> {
    if limit == 0 || ids.len() <= limit {
        return ids.to_vec();
    }
    match method.to_uppercase().as_str() {
        LIMIT_FIFO => ids[ids.len() - limit..].to_vec(),
        _ => ids[..limit].to_vec(),
    }
}

/// `make_relation_chunk_key` (`utils.py:7383`): the sorted pair joined by the
/// graph field separator — the undirected relation identity.
pub fn make_relation_chunk_key(src: &str, tgt: &str) -> String {
    let mut parts = [src, tgt];
    parts.sort();
    parts.join(GRAPH_FIELD_SEP)
}

/// `parse_relation_chunk_key` (`utils.py:7390`).
pub fn parse_relation_chunk_key(key: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = key.split(GRAPH_FIELD_SEP).collect();
    match parts.as_slice() {
        [src, tgt] => Some((src.to_string(), tgt.to_string())),
        _ => None,
    }
}

/// Merge new tracking entries into the truth layer for one insert.
///
/// The reference unions the existing and incoming id lists, dedups
/// first-seen-first, caps the result, and only touches rows that actually
/// changed — an unchanged row is not rewritten, so an unrelated graph element
/// keeps a byte-identical stored row (stage doc T1 judgement criterion).
pub async fn merge_tracking_rows(
    entity_chunks: &dyn KvStore,
    relation_chunks: &dyn KvStore,
    entity_rows: &BTreeMap<String, Vec<String>>,
    relation_rows: &BTreeMap<String, Vec<String>>,
    source_ids_limit: usize,
    limit_method: &str,
) -> crate::core::traits::Result<()> {
    let mut upserts: Vec<(String, Value)> = Vec::new();
    for (entity, new_ids) in entity_rows {
        let existing = read_ids(entity_chunks, entity).await?;
        let merged = merge_source_ids(&existing, new_ids, source_ids_limit, limit_method);
        if merged != existing {
            upserts.push((entity.clone(), json!({ "chunks": merged })));
        }
    }
    if !upserts.is_empty() {
        entity_chunks.upsert(upserts).await?;
    }

    let mut upserts: Vec<(String, Value)> = Vec::new();
    for (relation, new_ids) in relation_rows {
        let existing = read_ids(relation_chunks, relation).await?;
        let merged = merge_source_ids(&existing, new_ids, source_ids_limit, limit_method);
        if merged != existing {
            upserts.push((relation.clone(), json!({ "chunks": merged })));
        }
    }
    if !upserts.is_empty() {
        relation_chunks.upsert(upserts).await?;
    }
    Ok(())
}

/// `merge_source_ids` (`utils.py:7183-7243`): union with the existing list
/// first, then apply the cap.
pub fn merge_source_ids(
    existing: &[String],
    incoming: &[String],
    limit: usize,
    method: &str,
) -> Vec<String> {
    let mut merged: Vec<String> = Vec::with_capacity(existing.len() + incoming.len());
    for id in existing.iter().chain(incoming) {
        if !id.is_empty() && !merged.contains(id) {
            merged.push(id.clone());
        }
    }
    apply_source_ids_limit(&merged, limit, method)
}

async fn read_ids(store: &dyn KvStore, key: &str) -> crate::core::traits::Result<Vec<String>> {
    let row = store.get_by_id(key).await?;
    Ok(row
        .and_then(|value| value.get("chunks").cloned())
        .and_then(|chunks| serde_json::from_value::<Vec<String>>(chunks).ok())
        .unwrap_or_default())
}

/// Queue a tracking row for repair when its truth write fails (`RepairTarget`).
pub async fn queue_tracking_row(
    repair: &RepairQueue,
    target: crate::store::repair::RepairTarget,
    key: &str,
    chunks: &[String],
) -> crate::core::traits::Result<()> {
    repair
        .record(target, key, json!({ "chunks": chunks }))
        .await
}

/// Handle for the two tracking stores, so the pipeline can carry one field.
#[derive(Clone)]
pub struct TrackingStores {
    pub entity_chunks: Arc<dyn KvStore>,
    pub relation_chunks: Arc<dyn KvStore>,
}
