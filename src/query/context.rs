//! Context rendering — `_build_context_str` (`operate.py:5873`).
//!
//! Three JSON blocks (entities, relations, chunks) plus the reference document
//! list, all rendered into `PROMPTS['kg_query_context']`. The chunk ids in the
//! blocks are `DC1..DCn`, and the reference list maps those ids to the owning
//! document path — the model cites `[DCn]` and the answer carries provenance.

use serde_json::{json, Value};

/// One entity row in the context block (`_apply_token_truncation`).
#[derive(Debug, Clone, PartialEq)]
pub struct EntityRow {
    pub entity: String,
    pub entity_type: String,
    pub description: String,
    pub created_at: String,
    pub file_path: String,
}

impl EntityRow {
    pub fn to_json(&self) -> Value {
        json!({
            "entity": self.entity,
            "type": self.entity_type,
            "description": self.description,
            "created_at": self.created_at,
            "file_path": self.file_path,
        })
    }
}

/// One relation row in the context block.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationRow {
    pub entity1: String,
    pub entity2: String,
    pub description: String,
    pub created_at: String,
    pub file_path: String,
}

impl RelationRow {
    pub fn to_json(&self) -> Value {
        json!({
            "entity1": self.entity1,
            "entity2": self.entity2,
            "description": self.description,
            "created_at": self.created_at,
            "file_path": self.file_path,
        })
    }
}

/// One chunk row: `DC{i}` id plus content and owning document.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRow {
    pub id: String,
    pub content: String,
    pub file_path: String,
    pub chunk_order_index: usize,
}

impl ChunkRow {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "content": self.content,
            "file_path": self.file_path,
            "chunk_order_index": self.chunk_order_index,
        })
    }
}

/// The four rendered sections.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryContext {
    pub entities: Vec<EntityRow>,
    pub relations: Vec<RelationRow>,
    pub chunks: Vec<ChunkRow>,
    /// `file_path` per distinct document, in first-seen order.
    pub references: Vec<String>,
}

impl QueryContext {
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty() && self.relations.is_empty() && self.chunks.is_empty()
    }

    /// The `DC{i}` ids for the chunk block, in order.
    pub fn chunk_ids(&self) -> Vec<String> {
        self.chunks.iter().map(|chunk| chunk.id.clone()).collect()
    }

    /// Render the final LLM context string (`_build_context_str` tail).
    pub fn render(&self) -> crate::core::text::TextResult<String> {
        let entities_str = serde_json::to_string_pretty(
            &self
                .entities
                .iter()
                .map(EntityRow::to_json)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        let relations_str = serde_json::to_string_pretty(
            &self
                .relations
                .iter()
                .map(RelationRow::to_json)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        let chunks_str = serde_json::to_string_pretty(
            &self
                .chunks
                .iter()
                .map(ChunkRow::to_json)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        let reference_list_str = self
            .references
            .iter()
            .enumerate()
            .map(|(index, path)| format!("[{}] {}", index + 1, path))
            .collect::<Vec<_>>()
            .join("\n");
        crate::core::text::fill_template(
            crate::graph::prompts::KG_QUERY_CONTEXT,
            &[
                ("entities_str", entities_str.as_str()),
                ("relations_str", relations_str.as_str()),
                ("text_chunks_str", chunks_str.as_str()),
                ("reference_list_str", reference_list_str.as_str()),
            ],
        )
    }
}

/// Assign the `DC{i}` ids in order; the reference numbers after all truncation
/// and dedup, so the ids are stable for the truncated set.
pub fn assign_chunk_ids(rows: Vec<ChunkRow>) -> Vec<ChunkRow> {
    rows.into_iter()
        .enumerate()
        .map(|(index, mut row)| {
            row.id = format!("DC{}", index + 1);
            row
        })
        .collect()
}

/// Distinct file paths in first-seen order — the reference list.
pub fn reference_list(rows: &[ChunkRow]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for row in rows {
        if !row.file_path.is_empty() && !seen.contains(&row.file_path) {
            seen.push(row.file_path.clone());
        }
    }
    seen
}

/// Format a unix timestamp the way the reference renders `created_at`
/// (`_apply_token_truncation`: `time.strftime("%Y-%m-%d %H:%M:%S",
/// time.localtime(...))`).
pub fn format_created_at(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "UNKNOWN".to_string();
    }
    crate::core::text::format_local_datetime(timestamp)
}
