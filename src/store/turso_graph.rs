//! Turso graph store — the durable graph arm of R1, mirroring `MemoryGraph`
//! semantics (`BaseGraphStorage`, base.py:117-186).
//!
//! Two adjacency tables: `graph_node(scope, id, props)` and
//! `graph_edge(scope, src, dst, props)`, both primary-keyed so `upsert` is an
//! `INSERT OR REPLACE` and the same (scope, id) always means one logical row.
//! `scope` isolates graphs that share a database file.
//!
//! Why not a graph engine (kuzu): see the deviation note in
//! `todo/01-nano-graphrag-port.md` — the bundled static build collides with
//! `turso_core`'s simsimd at link time, and R1 needs exactly the `GraphStore`
//! surface, not multi-hop/PageRank (R2 re-evaluates).
//!
//! Derivative: drop the tables and rebuild from the truth layer.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as Json;
use tokio::sync::Mutex;
use turso::{params, Builder, Connection, Value as TursoValue};

use crate::core::traits::{GraphSnapshot, GraphStore, Result, StoreError};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS graph_node (
     scope TEXT NOT NULL,
     id TEXT NOT NULL,
     props TEXT NOT NULL,
     PRIMARY KEY (scope, id)
 );
 CREATE TABLE IF NOT EXISTS graph_edge (
     scope TEXT NOT NULL,
     src TEXT NOT NULL,
     dst TEXT NOT NULL,
     props TEXT NOT NULL,
     PRIMARY KEY (scope, src, dst)
 )";

pub struct TursoGraph {
    _db: Arc<turso::Database>,
    conn: Arc<Mutex<Connection>>,
    scope: String,
}

impl TursoGraph {
    /// Open (creating if needed) a database file and both graph tables.
    pub async fn open(path: &str, scope: impl Into<String>) -> Result<Self> {
        let db = Builder::new_local(path).build().await.map_err(backend)?;
        let conn = db.connect().map_err(backend)?;
        conn.execute_batch(SCHEMA).await.map_err(backend)?;
        Ok(Self {
            _db: Arc::new(db),
            conn: Arc::new(Mutex::new(conn)),
            scope: scope.into(),
        })
    }
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn text_value(value: TursoValue) -> Result<String> {
    match value {
        TursoValue::Text(text) => Ok(text),
        other => Err(StoreError::Backend(format!(
            "expected text column, got {other:?}"
        ))),
    }
}

fn int_value(value: TursoValue) -> Result<i64> {
    match value {
        TursoValue::Integer(number) => Ok(number),
        other => Err(StoreError::Backend(format!(
            "expected integer column, got {other:?}"
        ))),
    }
}

fn decode(value: TursoValue) -> Result<Json> {
    serde_json::from_str(&text_value(value)?).map_err(backend)
}

/// `MemoryGraph` merge: object attributes merge key-by-key (last write wins),
/// anything else replaces wholesale.
fn merge_attributes(existing: Option<&Json>, incoming: &Json) -> Json {
    match (existing.and_then(Json::as_object), incoming.as_object()) {
        (Some(old), Some(new)) => {
            let mut merged = old.clone();
            for (key, value) in new {
                merged.insert(key.clone(), value.clone());
            }
            Json::Object(merged)
        }
        _ => incoming.clone(),
    }
}

/// Reads on a locked connection: the write paths hold the connection (single
/// turso connection means one lock) and cannot await a second acquisition.
async fn read_node(conn: &Connection, scope: &str, node_id: &str) -> Result<Option<Json>> {
    let mut rows = conn
        .query(
            "SELECT props FROM graph_node WHERE scope = ?1 AND id = ?2",
            params![scope.to_string(), node_id.to_string()],
        )
        .await
        .map_err(backend)?;
    match rows.next().await.map_err(backend)? {
        Some(row) => Ok(Some(decode(row.get_value(0).map_err(backend)?)?)),
        None => Ok(None),
    }
}

async fn read_edge(conn: &Connection, scope: &str, src: &str, tgt: &str) -> Result<Option<Json>> {
    let mut rows = conn
        .query(
            "SELECT props FROM graph_edge WHERE scope = ?1 AND src = ?2 AND dst = ?3",
            params![scope.to_string(), src.to_string(), tgt.to_string()],
        )
        .await
        .map_err(backend)?;
    match rows.next().await.map_err(backend)? {
        Some(row) => Ok(Some(decode(row.get_value(0).map_err(backend)?)?)),
        None => Ok(None),
    }
}

#[async_trait]
impl GraphStore for TursoGraph {
    async fn has_node(&self, node_id: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM graph_node WHERE scope = ?1 AND id = ?2 LIMIT 1",
                params![self.scope.clone(), node_id.to_string()],
            )
            .await
            .map_err(backend)?;
        Ok(rows.next().await.map_err(backend)?.is_some())
    }

    async fn has_edge(&self, src: &str, tgt: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM graph_edge WHERE scope = ?1 AND src = ?2 AND dst = ?3 LIMIT 1",
                params![self.scope.clone(), src.to_string(), tgt.to_string()],
            )
            .await
            .map_err(backend)?;
        Ok(rows.next().await.map_err(backend)?.is_some())
    }

    async fn get_node(&self, node_id: &str) -> Result<Option<Json>> {
        let conn = self.conn.lock().await;
        read_node(&conn, &self.scope, node_id).await
    }

    async fn get_nodes_batch(&self, ids: &[String]) -> Result<Vec<Option<Json>>> {
        let conn = self.conn.lock().await;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(read_node(&conn, &self.scope, id).await?);
        }
        Ok(out)
    }

    async fn get_edge(&self, src: &str, tgt: &str) -> Result<Option<Json>> {
        let conn = self.conn.lock().await;
        read_edge(&conn, &self.scope, src, tgt).await
    }

    async fn get_edges_batch(&self, pairs: &[(String, String)]) -> Result<Vec<Option<Json>>> {
        let conn = self.conn.lock().await;
        let mut out = Vec::with_capacity(pairs.len());
        for (src, tgt) in pairs {
            out.push(read_edge(&conn, &self.scope, src, tgt).await?);
        }
        Ok(out)
    }

    async fn node_degree(&self, node_id: &str) -> Result<i64> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM graph_edge WHERE scope = ?1 AND (src = ?2 OR dst = ?2)",
                params![self.scope.clone(), node_id.to_string()],
            )
            .await
            .map_err(backend)?;
        match rows.next().await.map_err(backend)? {
            Some(row) => int_value(row.get_value(0).map_err(backend)?),
            None => Ok(0),
        }
    }

    async fn node_degrees_batch(&self, ids: &[String]) -> Result<Vec<i64>> {
        // Each call takes the single connection itself.
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.node_degree(id).await?);
        }
        Ok(out)
    }

    async fn edge_degree(&self, src: &str, tgt: &str) -> Result<i64> {
        Ok(self.node_degree(src).await? + self.node_degree(tgt).await?)
    }

    async fn edge_degrees_batch(&self, pairs: &[(String, String)]) -> Result<Vec<i64>> {
        let mut out = Vec::with_capacity(pairs.len());
        for (src, tgt) in pairs {
            out.push(self.edge_degree(src, tgt).await?);
        }
        Ok(out)
    }

    async fn node_edges(&self, node_id: &str) -> Result<Option<Vec<(String, String)>>> {
        let conn = self.conn.lock().await;
        let has_node = conn
            .query(
                "SELECT 1 FROM graph_node WHERE scope = ?1 AND id = ?2 LIMIT 1",
                params![self.scope.clone(), node_id.to_string()],
            )
            .await
            .map_err(backend)?
            .next()
            .await
            .map_err(backend)?
            .is_some();
        if !has_node {
            return Ok(None);
        }
        let mut rows = conn
            .query(
                "SELECT src, dst FROM graph_edge WHERE scope = ?1 AND (src = ?2 OR dst = ?2)",
                params![self.scope.clone(), node_id.to_string()],
            )
            .await
            .map_err(backend)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(backend)? {
            let src = text_value(row.get_value(0).map_err(backend)?)?;
            let dst = text_value(row.get_value(1).map_err(backend)?)?;
            // `MemoryGraph` always puts the queried node first in the pair.
            if src == node_id {
                out.push((src, dst));
            } else {
                out.push((dst, src));
            }
        }
        Ok(Some(out))
    }

    async fn nodes_edges_batch(
        &self,
        ids: &[String],
    ) -> Result<Vec<Option<Vec<(String, String)>>>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.node_edges(id).await?);
        }
        Ok(out)
    }

    async fn upsert_node(&self, node_id: &str, data: Json) -> Result<()> {
        let conn = self.conn.lock().await;
        let existing = read_node(&conn, &self.scope, node_id).await?;
        let merged = merge_attributes(existing.as_ref(), &data);
        conn.execute(
            "INSERT OR REPLACE INTO graph_node (scope, id, props) VALUES (?1, ?2, ?3)",
            params![self.scope.clone(), node_id.to_string(), merged.to_string()],
        )
        .await
        .map(|_| ())
        .map_err(backend)
    }

    async fn upsert_nodes_batch(&self, rows: Vec<(String, Json)>) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("BEGIN").await.map_err(backend)?;
        for (id, data) in rows {
            let existing = read_node(&conn, &self.scope, &id).await?;
            let merged = merge_attributes(existing.as_ref(), &data);
            if let Err(error) = conn
                .execute(
                    "INSERT OR REPLACE INTO graph_node (scope, id, props) VALUES (?1, ?2, ?3)",
                    params![self.scope.clone(), id, merged.to_string()],
                )
                .await
            {
                conn.execute_batch("ROLLBACK").await.map_err(backend)?;
                return Err(backend(error));
            }
        }
        conn.execute_batch("COMMIT").await.map_err(backend)
    }

    async fn upsert_edge(&self, src: &str, tgt: &str, data: Json) -> Result<()> {
        let conn = self.conn.lock().await;
        let existing = read_edge(&conn, &self.scope, src, tgt).await?;
        let merged = merge_attributes(existing.as_ref(), &data);
        conn.execute(
            "INSERT OR REPLACE INTO graph_edge (scope, src, dst, props) VALUES (?1, ?2, ?3, ?4)",
            params![
                self.scope.clone(),
                src.to_string(),
                tgt.to_string(),
                merged.to_string()
            ],
        )
        .await
        .map(|_| ())
        .map_err(backend)
    }

    async fn upsert_edges_batch(&self, rows: Vec<(String, String, Json)>) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("BEGIN").await.map_err(backend)?;
        for (src, tgt, data) in rows {
            let existing = read_edge(&conn, &self.scope, &src, &tgt).await?;
            let merged = merge_attributes(existing.as_ref(), &data);
            if let Err(error) = conn
                .execute(
                    "INSERT OR REPLACE INTO graph_edge (scope, src, dst, props) VALUES (?1, ?2, ?3, ?4)",
                    params![self.scope.clone(), src, tgt, merged.to_string()],
                )
                .await
            {
                conn.execute_batch("ROLLBACK").await.map_err(backend)?;
                return Err(backend(error));
            }
        }
        conn.execute_batch("COMMIT").await.map_err(backend)
    }

    async fn remove_node(&self, node_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM graph_node WHERE scope = ?1 AND id = ?2",
            params![self.scope.clone(), node_id.to_string()],
        )
        .await
        .map_err(backend)?;
        conn.execute(
            "DELETE FROM graph_edge WHERE scope = ?1 AND (src = ?2 OR dst = ?2)",
            params![self.scope.clone(), node_id.to_string()],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn remove_edge(&self, src: &str, tgt: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM graph_edge WHERE scope = ?1 AND src = ?2 AND dst = ?3",
            params![self.scope.clone(), src.to_string(), tgt.to_string()],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn snapshot(&self) -> Result<GraphSnapshot> {
        let conn = self.conn.lock().await;
        let mut node_rows = conn
            .query(
                "SELECT id, props FROM graph_node WHERE scope = ?1",
                params![self.scope.clone()],
            )
            .await
            .map_err(backend)?;
        let mut nodes = Vec::new();
        while let Some(row) = node_rows.next().await.map_err(backend)? {
            let id = text_value(row.get_value(0).map_err(backend)?)?;
            nodes.push((id, decode(row.get_value(1).map_err(backend)?)?));
        }
        let mut edge_rows = conn
            .query(
                "SELECT src, dst, props FROM graph_edge WHERE scope = ?1",
                params![self.scope.clone()],
            )
            .await
            .map_err(backend)?;
        let mut edges = Vec::new();
        while let Some(row) = edge_rows.next().await.map_err(backend)? {
            let src = text_value(row.get_value(0).map_err(backend)?)?;
            let dst = text_value(row.get_value(1).map_err(backend)?)?;
            edges.push((src, dst, decode(row.get_value(2).map_err(backend)?)?));
        }
        Ok(GraphSnapshot { nodes, edges })
    }

    async fn index_done(&self) -> Result<()> {
        // Writes are committed per call; nothing to flush.
        Ok(())
    }
}
