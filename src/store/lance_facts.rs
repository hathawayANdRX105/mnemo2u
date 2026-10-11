//! LanceDB vector arm over fact embeddings (stage doc 03 T2).
//!
//! One Lance table `fact_vectors(ws, sess, task, fact_id, vector)` where the
//! three scope components are plain strings: an unconstrained component is
//! stored as the empty string (the same `None -> ""` normalization
//! `turso_lexical` applies, kept as a private copy in each arm), so a single
//! table holds every scope and `upsert` is a merge on the composite key
//! `(ws, sess, task, fact_id)` — one logical row per (scope, fact).
//!
//! `search` applies the scope predicate *inside* the vector query as a
//! **pre-filter** ([`ScopeKey::admits`] semantics, stage doc 03 contract 1):
//! `ws` is always an equality and `sess` / `task` are added only when the
//! caller constrained that component, so a narrow caller never sees another
//! scope's rows — not even their ranks. LanceDB runs a scalar filter before
//! the ANN scan by default (`prefilter` stays true unless `postfilter()` is
//! called, which this arm never does), so the scope gate is honored by the
//! search itself rather than as a post-filter over an unscoped top-k.
//!
//! Similarity口径: the reported f64 is cosine *similarity*, in `[-1, 1]`,
//! higher = more similar. LanceDB's `_distance` under `DistanceType::Cosine`
//! is `1 - cos(q, v)` (0 when the vectors agree, 2 when they oppose), so
//! `similarity = 1 - _distance`. Results are returned sorted by similarity
//! descending with `fact_id` ascending as the deterministic tie-break, then
//! truncated to `limit`.
//!
//! Rebuildable derived data: the fact truth layer owns rows; this index can
//! always be dropped and re-embedded.

use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
use arrow_array::{
    Array, Float32Array, Float64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::{connect, query::ExecutableQuery, query::QueryBase, DistanceType, Table};
use tokio::sync::OnceCell;

use crate::core::text::md5_hex;
use crate::core::traits::{FactVectorStore, Result, StoreError};
use crate::core::types::{FactId, ScopeKey};

const WS: &str = "ws";
const SESS: &str = "sess";
const TASK: &str = "task";
const FACT_ID: &str = "fact_id";
const VECTOR: &str = "vector";
const DISTANCE: &str = "_distance";
const TABLE: &str = "fact_vectors";

/// One-table LanceDB vector store over fact embeddings, scope-gated by a
/// pre-filter inside every ANN query (stage doc 03 T2).
pub struct LanceFactVector {
    uri: String,
    dim: usize,
    table: OnceCell<Table>,
}

impl LanceFactVector {
    /// Open (creating if needed) the fact-vector table under `path` for
    /// `dim`-dimensional embeddings. `path` is a local directory (or a
    /// LanceDB URI); `dim` is the fixed vector width the table is built with.
    pub async fn new(path: &str, dim: usize) -> Result<Self> {
        if dim == 0 {
            return Err(StoreError::Backend(
                "fact-vector table requires dim > 0".into(),
            ));
        }
        let store = Self {
            uri: path.to_string(),
            dim,
            table: OnceCell::new(),
        };
        store.table_handle().await?;
        Ok(store)
    }

    fn schema(dim: usize) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(WS, DataType::Utf8, false),
            Field::new(SESS, DataType::Utf8, false),
            Field::new(TASK, DataType::Utf8, false),
            Field::new(FACT_ID, DataType::Utf8, false),
            Field::new(
                VECTOR,
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dim as i32,
                ),
                true,
            ),
        ]))
    }

    /// The table handle is cached: a store upserts per fact and re-connecting
    /// per call would dominate the write time (mirror of `lancedb.rs`).
    async fn table_handle(&self) -> Result<Table> {
        let handle = self
            .table
            .get_or_try_init(|| async {
                let conn = connect(&self.uri).execute().await.map_err(backend)?;
                let exists = conn
                    .table_names()
                    .execute()
                    .await
                    .map_err(backend)?
                    .into_iter()
                    .any(|name| name == TABLE);
                if exists {
                    conn.open_table(TABLE).execute().await.map_err(backend)
                } else {
                    conn.create_empty_table(TABLE, Self::schema(self.dim))
                        .execute()
                        .await
                        .map_err(backend)
                }
            })
            .await?;
        Ok(handle.clone())
    }

    /// One-row batch for an upsert: the (scope, id) key plus its vector.
    async fn upsert_batch(
        &self,
        scope: &ScopeKey,
        id: &FactId,
        embedding: &[f32],
    ) -> Result<RecordBatch> {
        if embedding.len() != self.dim {
            return Err(StoreError::Backend(format!(
                "embedding has {} dims, table declares {}",
                embedding.len(),
                self.dim
            )));
        }
        let mut ws = StringBuilder::new();
        ws.append_value(&scope.workspace_id);
        let mut sess = StringBuilder::new();
        sess.append_value(norm(&scope.session_id));
        let mut task = StringBuilder::new();
        task.append_value(norm(&scope.task_id));
        let mut fact_id = StringBuilder::new();
        fact_id.append_value(&id.0);
        let mut vector = FixedSizeListBuilder::new(Float32Builder::new(), self.dim as i32);
        for value in embedding {
            vector.values().append_value(*value);
        }
        vector.append(true);
        let schema = Self::schema(self.dim);
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(ws.finish()),
                Arc::new(sess.finish()),
                Arc::new(task.finish()),
                Arc::new(fact_id.finish()),
                Arc::new(vector.finish()),
            ],
        )
        .map_err(backend)
    }

    /// The SQL equality predicate for a scope: `ws` is always present, and
    /// `sess` / `task` are appended only when the caller constrained that
    /// component — the store-side rendering of [`ScopeKey::admits`].
    fn scope_predicate(&self, scope: &ScopeKey) -> String {
        let mut sql = format!("{} = '{}'", WS, sql_quote(&scope.workspace_id));
        if let Some(session) = &scope.session_id {
            sql.push_str(&format!(" AND {} = '{}'", SESS, sql_quote(session)));
        }
        if let Some(task) = &scope.task_id {
            sql.push_str(&format!(" AND {} = '{}'", TASK, sql_quote(task)));
        }
        sql
    }
}

/// Quote a value for an SQL string literal in the scope predicate: double the
/// single quotes so a caller-supplied identifier cannot close the literal.
fn sql_quote(value: &str) -> String {
    value.replace('\'', "''")
}

/// Map any backend error onto the store error surface (repo rule: errors are
/// always [`StoreError`], wrapped by `map_err`).
fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// An unconstrained scope component (`None`) is stored as the empty string so
/// every row is one key cell; `search` then compares plain values and never
/// reasons about NULL. `Some("")` normalizes to the same cell.
fn norm(component: &Option<String>) -> String {
    component.clone().unwrap_or_default()
}

/// Stage doc 03 T2 embedding cache key: a stable hash of the normalized scope,
/// the embedding `model_id`, and the embedded `text` — `hash(scope, model,
/// text)`. The scope's `session_id` / `task_id` are normalized through
/// [`norm`] before hashing, so `None` and `Some("")` are indistinguishable
/// (both an unconstrained component) while every other difference in any of
/// the five components changes the key.
pub fn fact_embedding_cache_key(scope: &ScopeKey, model_id: &str, text: &str) -> String {
    let canonical = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        scope.workspace_id,
        norm(&scope.session_id),
        norm(&scope.task_id),
        model_id,
        text
    );
    format!("evf-{}", md5_hex(&canonical))
}

#[async_trait]
impl FactVectorStore for LanceFactVector {
    /// Insert, or replace in place, the vector for `(scope, id)`. A merge on
    /// the composite key means the same (scope, id) never accumulates two
    /// rows — a re-embed of the same fact updates its vector.
    async fn upsert(&self, scope: &ScopeKey, id: &FactId, embedding: &[f32]) -> Result<()> {
        let table = self.table_handle().await?;
        let batch = self.upsert_batch(scope, id, embedding).await?;
        let schema = table.schema().await.map_err(backend)?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let mut merge = table.merge_insert(&[WS, SESS, TASK, FACT_ID]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge.execute(Box::new(reader)).await.map_err(backend)?;
        Ok(())
    }

    /// Drop the `(scope, id)` row. A scope that never held `id` is not an
    /// error — recall arms drop ids speculatively, so a non-matching
    /// predicate simply deletes zero rows.
    async fn remove(&self, scope: &ScopeKey, id: &FactId) -> Result<()> {
        let table = self.table_handle().await?;
        let mut predicate = self.scope_predicate(scope);
        predicate.push_str(&format!(" AND {} = '{}'", FACT_ID, sql_quote(&id.0)));
        table.delete(&predicate).await.map_err(backend)?;
        Ok(())
    }

    /// ANN top-`limit` for `query` within `scope`. The scope predicate is a
    /// pre-filter applied *inside* the vector search, never a post-filter.
    /// Returns `(FactId, similarity)` sorted by similarity descending with
    /// `fact_id` ascending as the tie-break, truncated to `limit`.
    async fn search(
        &self,
        scope: &ScopeKey,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<(FactId, f64)>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        if query.len() != self.dim {
            return Err(StoreError::Backend(format!(
                "query has {} dims, table declares {}",
                query.len(),
                self.dim
            )));
        }
        let table = self.table_handle().await?;
        let predicate = self.scope_predicate(scope);
        let batches = table
            .query()
            .nearest_to(query)
            .map_err(backend)?
            .only_if(predicate)
            .distance_type(DistanceType::Cosine)
            .limit(limit)
            .execute()
            .await
            .map_err(backend)?
            .try_collect::<Vec<RecordBatch>>()
            .await
            .map_err(backend)?;

        let mut hits: Vec<(FactId, f64)> = Vec::new();
        for batch in &batches {
            read_hits(batch, &mut hits)?;
        }
        // Similarity descending; fact_id ascending as the deterministic
        // tie-break so equal-scoring rows never reorder between runs.
        hits.sort_by(|a, b| {
            let by_sim = b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal);
            by_sim.then_with(|| a.0 .0.cmp(&b.0 .0))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

/// Read `(fact_id, similarity)` rows from a result batch. `_distance` may be
/// f64 or f32 depending on the index, so read both; a missing or null value
/// defaults to 0.0 distance (similarity 1.0), mirroring `lancedb.rs`.
fn read_hits(batch: &RecordBatch, out: &mut Vec<(FactId, f64)>) -> Result<()> {
    let ids = batch
        .column_by_name(FACT_ID)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| StoreError::Backend("search result carries no fact_id column".into()))?;
    let wide = batch
        .column_by_name(DISTANCE)
        .and_then(|column| column.as_any().downcast_ref::<Float64Array>());
    let narrow = batch
        .column_by_name(DISTANCE)
        .and_then(|column| column.as_any().downcast_ref::<Float32Array>());

    for row in 0..batch.num_rows() {
        if ids.is_null(row) {
            continue;
        }
        let distance = match (wide, narrow) {
            (Some(values), _) if !values.is_null(row) => values.value(row) as f32,
            (_, Some(values)) if !values.is_null(row) => values.value(row),
            _ => 0.0,
        };
        let similarity = 1.0 - distance as f64;
        out.push((FactId(ids.value(row).to_string()), similarity));
    }
    Ok(())
}
