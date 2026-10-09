//! LanceDB vector store — the vector arm for `chunks_vdb` / `entities_vdb`
//! (`VectorStore` trait).
//!
//! One Lance table per namespace (`id`, `content`, `vector`, `meta`); `upsert`
//! is a merge on `id` so repeated extraction runs update in place, matching the
//! reference's `text_index.replace` / `entity_index.replace` behaviour.
//!
//! Rebuildable derived data: the truth layer owns rows, this index can always
//! be dropped and re-embedded.

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

use crate::core::traits::{Embedder, Result, StoreError, VectorHit, VectorRow, VectorStore};

const ID: &str = "id";
const CONTENT: &str = "content";
const VECTOR: &str = "vector";
const META: &str = "meta";
const DISTANCE: &str = "_distance";

/// Cosine-thresholded vector search over LanceDB. Mirrors
/// `NanoVectorDBStorage` semantics: similarity threshold, top-k, `_distance`
/// reported as cosine similarity.
pub struct LanceVector {
    uri: String,
    namespace: String,
    embedder: Arc<dyn Embedder>,
    cosine_threshold: f32,
    table: OnceCell<Table>,
}

impl LanceVector {
    /// Open (creating if needed) the table for `namespace` under `uri`.
    pub async fn open(
        uri: impl Into<String>,
        namespace: impl Into<String>,
        embedder: Arc<dyn Embedder>,
        cosine_threshold: f32,
    ) -> Result<Self> {
        let store = Self {
            uri: uri.into(),
            namespace: namespace.into(),
            embedder,
            cosine_threshold,
            table: OnceCell::new(),
        };
        store.table_handle().await?;
        Ok(store)
    }

    fn schema(dim: usize) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(ID, DataType::Utf8, false),
            Field::new(CONTENT, DataType::Utf8, true),
            Field::new(
                VECTOR,
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dim as i32,
                ),
                true,
            ),
            Field::new(META, DataType::Utf8, true),
        ]))
    }

    /// The table handle is cached: the pipeline upserts per chunk batch and
    /// re-connecting per call would dominate the write time.
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
                    .any(|name| name == self.namespace);
                if exists {
                    conn.open_table(&self.namespace)
                        .execute()
                        .await
                        .map_err(backend)
                } else {
                    conn.create_empty_table(&self.namespace, Self::schema(self.embedder.dim()))
                        .execute()
                        .await
                        .map_err(backend)
                }
            })
            .await?;
        Ok(handle.clone())
    }

    async fn batch(&self, rows: &[VectorRow]) -> Result<RecordBatch> {
        let embeddings = self.embedder.embed(&to_texts(rows)).await?;
        let mut ids = StringBuilder::new();
        let mut contents = StringBuilder::new();
        let mut metas = StringBuilder::new();
        let mut vectors =
            FixedSizeListBuilder::new(Float32Builder::new(), self.embedder.dim() as i32);
        for (row, embedding) in rows.iter().zip(embeddings) {
            if embedding.len() != self.embedder.dim() {
                return Err(StoreError::Backend(format!(
                    "embedder {} returned {} dims, declared {}",
                    self.embedder.model_id(),
                    embedding.len(),
                    self.embedder.dim()
                )));
            }
            ids.append_value(&row.id);
            contents.append_value(&row.content);
            metas.append_value(row.meta.to_string());
            for value in embedding {
                vectors.values().append_value(value);
            }
            vectors.append(true);
        }
        let schema = Self::schema(self.embedder.dim());
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(ids.finish()),
                Arc::new(contents.finish()),
                Arc::new(vectors.finish()),
                Arc::new(metas.finish()),
            ],
        )
        .map_err(backend)
    }
}

fn to_texts(rows: &[VectorRow]) -> Vec<String> {
    rows.iter().map(|row| row.content.clone()).collect()
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

#[async_trait]
impl VectorStore for LanceVector {
    async fn upsert(&self, rows: Vec<VectorRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let table = self.table_handle().await?;
        let schema = table.schema().await.map_err(backend)?;
        let batch = self.batch(&rows).await?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let mut merge = table.merge_insert(&[ID]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge.execute(Box::new(reader)).await.map_err(backend)?;
        Ok(())
    }

    async fn query(&self, query: &str, top_k: usize) -> Result<Vec<VectorHit>> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        let table = self.table_handle().await?;
        let embedding = self.embedder.embed(&[query.to_string()]).await?.remove(0);
        if embedding.len() != self.embedder.dim() {
            return Err(StoreError::Backend(format!(
                "embedder {} returned {} dims, declared {}",
                self.embedder.model_id(),
                embedding.len(),
                self.embedder.dim()
            )));
        }
        let batches = table
            .query()
            .nearest_to(embedding)
            .map_err(backend)?
            .distance_type(DistanceType::Cosine)
            .limit(top_k)
            .execute()
            .await
            .map_err(backend)?
            .try_collect::<Vec<RecordBatch>>()
            .await
            .map_err(backend)?;

        let mut hits = Vec::new();
        for batch in &batches {
            hits.extend(read_batch(batch, self.cosine_threshold)?);
        }
        hits.sort_by(|a, b| {
            b.distance
                .partial_cmp(&a.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        Ok(hits)
    }

    async fn index_done(&self) -> Result<()> {
        Ok(())
    }
}

fn read_batch(batch: &RecordBatch, threshold: f32) -> Result<Vec<VectorHit>> {
    let ids = batch
        .column_by_name(ID)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| StoreError::Backend("query result carries no id column".into()))?;
    let metas = batch
        .column_by_name(META)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| StoreError::Backend("query result carries no meta column".into()))?;
    // LanceDB reports the metric as f64 or f32 depending on the index; the
    // distance we expose is cosine *similarity* (1 - cosine distance).
    let wide = batch
        .column_by_name(DISTANCE)
        .and_then(|column| column.as_any().downcast_ref::<Float64Array>());
    let narrow = batch
        .column_by_name(DISTANCE)
        .and_then(|column| column.as_any().downcast_ref::<Float32Array>());

    let mut hits = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if ids.is_null(row) {
            continue;
        }
        let distance = match (wide, narrow) {
            (Some(values), _) if !values.is_null(row) => 1.0 - values.value(row) as f32,
            (_, Some(values)) if !values.is_null(row) => 1.0 - values.value(row),
            _ => 1.0,
        };
        if distance < threshold {
            continue;
        }
        let meta = if metas.is_null(row) {
            serde_json::Value::Null
        } else {
            serde_json::from_str(metas.value(row)).map_err(backend)?
        };
        hits.push(VectorHit {
            id: ids.value(row).to_string(),
            distance,
            meta,
        });
    }
    Ok(hits)
}
