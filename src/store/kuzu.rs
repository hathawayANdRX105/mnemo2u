//! Kuzu graph store (feature `kuzu-backend`) — the graph arm of R1, mirroring
//! `MemoryGraph` semantics (`BaseGraphStorage`, base.py:117-186).
//!
//! Mapping: one node table keyed by the opaque node id and one rel table for the
//! edges; node/edge attribute payloads are stored as JSON strings because the
//! payload schema is data-driven (`entity_type`/`weight`/`clusters`…), not a fixed
//! column set.
//!
//! Two documented divergences from `MemoryGraph`:
//!
//! * An edge implies its endpoints. Kuzu has no dangling rels, so `upsert_edge`
//!   creates missing endpoint nodes with empty props (nx's `add_edge` does the
//!   same).
//! * Attribute merges happen in Rust (read-modify-write) under the write lock,
//!   so concurrent upserts of the same node cannot interleave writes.
//!
//! Derivative: kuzu is rebuilt from the truth layer, `drop` + re-`open` is always
//! available.

use async_trait::async_trait;
use kuzu::{Connection, Database, SystemConfig, Value};
use serde_json::Value as Json;
use tokio::sync::Mutex;

use crate::core::traits::{GraphSnapshot, GraphStore, Result, StoreError};

const CREATE_NODE_TABLE: &str =
    "CREATE NODE TABLE IF NOT EXISTS MnemoNode(id STRING, props STRING, PRIMARY KEY(id))";
const CREATE_REL_TABLE: &str =
    "CREATE REL TABLE IF NOT EXISTS MnemoEdge(FROM MnemoNode TO MnemoNode, props STRING)";

const UPSERT_NODE: &str = "MERGE (node:MnemoNode {id: $id}) SET node.props = $props";
const UPSERT_EDGE: &str = "\
MERGE (src:MnemoNode {id: $src}) \
MERGE (tgt:MnemoNode {id: $tgt}) \
MERGE (src)-[rel:MnemoEdge]->(tgt) SET rel.props = $props";
const HAS_NODE: &str = "MATCH (node:MnemoNode {id: $id}) RETURN count(node)";
const HAS_EDGE: &str =
    "MATCH (src:MnemoNode {id: $src})-[rel:MnemoEdge]->(tgt:MnemoNode {id: $tgt}) RETURN count(rel)";
const GET_NODE: &str = "MATCH (node:MnemoNode {id: $id}) RETURN node.props";
const GET_EDGE: &str =
    "MATCH (src:MnemoNode {id: $src})-[rel:MnemoEdge]->(tgt:MnemoNode {id: $tgt}) RETURN rel.props";
const OUT_DEGREE: &str = "MATCH (node:MnemoNode {id: $id})-[rel:MnemoEdge]->() RETURN count(rel)";
const IN_DEGREE: &str = "MATCH (node:MnemoNode {id: $id})<-[rel:MnemoEdge]-() RETURN count(rel)";
const OUT_NEIGHBOURS: &str =
    "MATCH (node:MnemoNode {id: $id})-[rel:MnemoEdge]->(peer) RETURN peer.id";
const IN_NEIGHBOURS: &str =
    "MATCH (node:MnemoNode {id: $id})<-[rel:MnemoEdge]-(peer) RETURN peer.id";
const SNAPSHOT_NODES: &str = "MATCH (node:MnemoNode) RETURN node.id, node.props";
const SNAPSHOT_EDGES: &str =
    "MATCH (src:MnemoNode)-[rel:MnemoEdge]->(tgt:MnemoNode) RETURN src.id, tgt.id, rel.props";

pub struct KuzuGraph {
    db: Database,
    /// Kuzu admits one write query at a time; serialise them here.
    write_lock: Mutex<()>,
}

impl KuzuGraph {
    pub async fn open(path: &str) -> Result<Self> {
        let db = Database::new(path, SystemConfig::default()).map_err(backend)?;
        {
            let connection = Connection::new(&db).map_err(backend)?;
            run(&connection, CREATE_NODE_TABLE, Vec::new())?;
            run(&connection, CREATE_REL_TABLE, Vec::new())?;
        }
        Ok(Self {
            db,
            write_lock: Mutex::new(()),
        })
    }

    fn connection(&self) -> Result<Connection<'_>> {
        Connection::new(&self.db).map_err(backend)
    }

    fn upsert_node_locked(&self, node_id: &str, data: Json) -> Result<()> {
        let merged = merge_attributes(self.read_node(node_id)?.as_ref(), &data);
        let connection = self.connection()?;
        let params = vec![
            ("id", Value::String(node_id.to_string())),
            ("props", Value::String(merged.to_string())),
        ];
        run(&connection, UPSERT_NODE, params).map(|_| ())
    }

    /// Sync read: the write paths run under the write lock and cannot await.
    fn read_node(&self, node_id: &str) -> Result<Option<Json>> {
        let connection = self.connection()?;
        let rows = run(
            &connection,
            GET_NODE,
            vec![("id", Value::String(node_id.to_string()))],
        )?;
        decode_payload(rows.first())
    }

    fn upsert_edge_locked(&self, src: &str, tgt: &str, data: Json) -> Result<()> {
        let connection = self.connection()?;
        let params = vec![
            ("src", Value::String(src.to_string())),
            ("tgt", Value::String(tgt.to_string())),
            ("props", Value::String(data.to_string())),
        ];
        run(&connection, UPSERT_EDGE, params).map(|_| ())
    }
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// Prepare + execute + collect all rows. `params` are bound by name so no query
/// text is ever assembled from data.
fn run(
    connection: &Connection,
    query: &str,
    params: Vec<(&str, Value)>,
) -> Result<Vec<Vec<Value>>> {
    let mut statement = connection.prepare(query).map_err(backend)?;
    let result = connection
        .execute(&mut statement, params)
        .map_err(backend)?;
    Ok(result.collect())
}

fn scalar_int(rows: &[Vec<Value>]) -> i64 {
    match rows.first().and_then(|row| row.first()) {
        Some(Value::Int64(value)) => *value,
        Some(Value::Int32(value)) => i64::from(*value),
        Some(Value::UInt64(value)) => *value as i64,
        _ => 0,
    }
}

fn string_at(row: &[Value], index: usize) -> Option<String> {
    match row.get(index) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

/// `MemoryGraph::upsert_node` merge: object attributes merge key-by-key (last
/// write wins), anything else replaces wholesale.
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

#[async_trait]
impl GraphStore for KuzuGraph {
    async fn has_node(&self, node_id: &str) -> Result<bool> {
        let connection = self.connection()?;
        let rows = run(
            &connection,
            HAS_NODE,
            vec![("id", Value::String(node_id.to_string()))],
        )?;
        Ok(scalar_int(&rows) > 0)
    }

    async fn has_edge(&self, src: &str, tgt: &str) -> Result<bool> {
        let connection = self.connection()?;
        let rows = run(
            &connection,
            HAS_EDGE,
            vec![
                ("src", Value::String(src.to_string())),
                ("tgt", Value::String(tgt.to_string())),
            ],
        )?;
        Ok(scalar_int(&rows) > 0)
    }

    async fn get_node(&self, node_id: &str) -> Result<Option<Json>> {
        self.read_node(node_id)
    }

    async fn get_nodes_batch(&self, ids: &[String]) -> Result<Vec<Option<Json>>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.get_node(id).await?);
        }
        Ok(out)
    }

    async fn get_edge(&self, src: &str, tgt: &str) -> Result<Option<Json>> {
        let connection = self.connection()?;
        let rows = run(
            &connection,
            GET_EDGE,
            vec![
                ("src", Value::String(src.to_string())),
                ("tgt", Value::String(tgt.to_string())),
            ],
        )?;
        decode_payload(rows.first())
    }

    async fn get_edges_batch(&self, pairs: &[(String, String)]) -> Result<Vec<Option<Json>>> {
        let mut out = Vec::with_capacity(pairs.len());
        for (src, tgt) in pairs {
            out.push(self.get_edge(src, tgt).await?);
        }
        Ok(out)
    }

    async fn node_degree(&self, node_id: &str) -> Result<i64> {
        let connection = self.connection()?;
        let outgoing = run(
            &connection,
            OUT_DEGREE,
            vec![("id", Value::String(node_id.to_string()))],
        )?;
        let incoming = run(
            &connection,
            IN_DEGREE,
            vec![("id", Value::String(node_id.to_string()))],
        )?;
        Ok(scalar_int(&outgoing) + scalar_int(&incoming))
    }

    async fn node_degrees_batch(&self, ids: &[String]) -> Result<Vec<i64>> {
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
        if !self.has_node(node_id).await? {
            return Ok(None);
        }
        let connection = self.connection()?;
        let id = Value::String(node_id.to_string());
        // `MemoryGraph` always puts the queried node first in the pair.
        let mut out = Vec::new();
        for peer in run(&connection, OUT_NEIGHBOURS, vec![("id", id.clone())])? {
            if let Some(peer_id) = string_at(&peer, 0) {
                out.push((node_id.to_string(), peer_id));
            }
        }
        for peer in run(&connection, IN_NEIGHBOURS, vec![("id", id)])? {
            if let Some(peer_id) = string_at(&peer, 0) {
                out.push((node_id.to_string(), peer_id));
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
        let _guard = self.write_lock.lock().await;
        self.upsert_node_locked(node_id, data)
    }

    async fn upsert_nodes_batch(&self, rows: Vec<(String, Json)>) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        for (id, data) in rows {
            self.upsert_node_locked(&id, data)?;
        }
        Ok(())
    }

    async fn upsert_edge(&self, src: &str, tgt: &str, data: Json) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.upsert_edge_locked(src, tgt, data)
    }

    async fn upsert_edges_batch(&self, rows: Vec<(String, String, Json)>) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        for (src, tgt, data) in rows {
            self.upsert_edge_locked(&src, &tgt, data)?;
        }
        Ok(())
    }

    async fn snapshot(&self) -> Result<GraphSnapshot> {
        let connection = self.connection()?;
        let mut snapshot = GraphSnapshot::default();
        for row in run(&connection, SNAPSHOT_NODES, Vec::new())? {
            let (Some(id), Some(payload)) = (string_at(&row, 0), string_at(&row, 1)) else {
                continue;
            };
            let data = serde_json::from_str(&payload).map_err(backend)?;
            snapshot.nodes.push((id, data));
        }
        for row in run(&connection, SNAPSHOT_EDGES, Vec::new())? {
            let (Some(src), Some(tgt), Some(payload)) =
                (string_at(&row, 0), string_at(&row, 1), string_at(&row, 2))
            else {
                continue;
            };
            let data = serde_json::from_str(&payload).map_err(backend)?;
            snapshot.edges.push((src, tgt, data));
        }
        Ok(snapshot)
    }

    async fn index_done(&self) -> Result<()> {
        Ok(())
    }
}

fn decode_payload(row: Option<&Vec<Value>>) -> Result<Option<Json>> {
    let Some(row) = row else { return Ok(None) };
    match string_at(row, 0) {
        Some(payload) => Ok(Some(serde_json::from_str(&payload).map_err(backend)?)),
        None => Ok(None),
    }
}
