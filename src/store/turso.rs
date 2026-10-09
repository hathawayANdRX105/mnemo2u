//! Turso KV store — the truth layer for R1: `full_docs`, `text_chunks`,
//! `llm_response_cache`, `community_reports` (`KvStore` trait).
//!
//! One table: `kv(namespace, key, value)` with the JSON-encoded value. The
//! reference uses one JSON file per namespace (`JsonKVStorage`); this is the
//! transactional replacement.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as Json;
use tokio::sync::Mutex;
use turso::{params, Builder, Connection, Database};

use crate::core::traits::{KvStore, Result, StoreError};

pub struct TursoKv {
    _db: Arc<Database>,
    conn: Arc<Mutex<Connection>>,
    namespace: String,
}

impl TursoKv {
    /// Open (creating if needed) a database file and make sure the KV table
    /// exists.
    pub async fn open(path: &str, namespace: impl Into<String>) -> Result<Self> {
        let db = Builder::new_local(path)
            .build()
            .await
            .map_err(backend_error)?;
        let conn = db.connect().map_err(backend_error)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (
                 namespace TEXT NOT NULL,
                 key TEXT NOT NULL,
                 value TEXT NOT NULL,
                 PRIMARY KEY (namespace, key)
             )",
        )
        .await
        .map_err(backend_error)?;
        Ok(Self {
            _db: Arc::new(db),
            conn: Arc::new(Mutex::new(conn)),
            namespace: namespace.into(),
        })
    }

    fn decode(value: &str) -> Result<Json> {
        serde_json::from_str(value).map_err(backend_error)
    }
}

fn backend_error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn text_value(value: turso::Value) -> Result<String> {
    match value {
        turso::Value::Text(text) => Ok(text),
        other => Err(StoreError::Backend(format!(
            "expected text column, got {other:?}"
        ))),
    }
}

#[async_trait]
impl KvStore for TursoKv {
    async fn all_keys(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT key FROM kv WHERE namespace = ?1",
                params![self.namespace.clone()],
            )
            .await
            .map_err(backend_error)?;
        let mut keys = Vec::new();
        while let Some(row) = rows.next().await.map_err(backend_error)? {
            keys.push(text_value(row.get_value(0).map_err(backend_error)?)?);
        }
        Ok(keys)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Json>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT value FROM kv WHERE namespace = ?1 AND key = ?2",
                params![self.namespace.clone(), id.to_string()],
            )
            .await
            .map_err(backend_error)?;
        match rows.next().await.map_err(backend_error)? {
            Some(row) => Ok(Some(Self::decode(&text_value(
                row.get_value(0).map_err(backend_error)?,
            )?)?)),
            None => Ok(None),
        }
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<Option<Json>>> {
        // Preserve input order; one query per id keeps the code boring.
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.get_by_id(id).await?);
        }
        Ok(out)
    }

    async fn filter_keys(&self, ids: &[String]) -> Result<Vec<String>> {
        let existing: HashSet<String> = self.all_keys().await?.into_iter().collect();
        Ok(ids
            .iter()
            .filter(|id| !existing.contains(*id))
            .cloned()
            .collect())
    }

    async fn upsert(&self, rows: Vec<(String, Json)>) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("BEGIN").await.map_err(backend_error)?;
        for (key, value) in rows {
            let encoded = serde_json::to_string(&value).map_err(backend_error)?;
            conn.execute(
                "INSERT OR REPLACE INTO kv (namespace, key, value) VALUES (?1, ?2, ?3)",
                params![self.namespace.clone(), key, encoded],
            )
            .await
            .map_err(backend_error)?;
        }
        conn.execute_batch("COMMIT").await.map_err(backend_error)?;
        Ok(())
    }

    async fn drop_all(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM kv WHERE namespace = ?1",
            params![self.namespace.clone()],
        )
        .await
        .map_err(backend_error)?;
        Ok(())
    }

    async fn remove(&self, keys: &[String]) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("BEGIN").await.map_err(backend_error)?;
        for key in keys {
            let statement = conn
                .execute(
                    "DELETE FROM kv WHERE namespace = ?1 AND key = ?2",
                    params![self.namespace.clone(), key.clone()],
                )
                .await;
            if statement.is_err() {
                conn.execute_batch("ROLLBACK")
                    .await
                    .map_err(backend_error)?;
            }
            statement.map_err(backend_error)?;
        }
        conn.execute_batch("COMMIT").await.map_err(backend_error)
    }

    async fn index_done(&self) -> Result<()> {
        // Writes are committed per call; nothing to flush.
        Ok(())
    }
}
