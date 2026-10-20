//! Redb graph store — the durable graph arm.
//!
//! Two tables in one redb file, keyed so a scope never mixes rows:
//! `nodes(scope, id) -> props_json` and `edges(scope, src, dst) -> props_json`.
//! The keys are redb tuples of `&str`, so lexicographic ordering gives the
//! adjacency prefix scan for free: every edge leaving `src` sits in one
//! contiguous range.
//!
//! Why redb rather than turso adjacency tables or a graph engine:
//! turso holds a single `Connection` behind a `tokio::Mutex`, so a graph call
//! used to serialise against the KV/fact writes on the *same* connection. A
//! separate redb file gives the graph its own lock, and the adjacency index
//! lives in memory — multi-hop traversal is pointer chasing, not a SQL
//! round-trip per hop. kuzu was the other candidate and is out for good: the
//! upstream repository is archived and its static link collides with
//! `turso_core`'s vendored simsimd (`todo/01-nano-graphrag-port.md` §8.5
//! records the link evidence; §8.7 records this switch).
//!
//! Semantics are `MemoryGraph`'s (`BaseGraphStorage`, base.py:117-186);
//! `tests/redb_graph.rs` pins that contract.
//!
//! Derivative: the file can be dropped and rebuilt from the truth layer —
//! the same "rebuildable derived index" rule the vector index follows.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde_json::Value as Json;
use tokio::sync::Mutex;

use crate::core::traits::{GraphSnapshot, GraphStore, Result, StoreError};

const NODES: TableDefinition<(&str, &str), &str> = TableDefinition::new("graph_node");
const EDGES: TableDefinition<(&str, &str, &str), &str> = TableDefinition::new("graph_edge");

/// In-memory adjacency: `src -> [(dst, seq)]` and `dst -> [(src, seq)]`, where
/// `seq` is the redb insertion order of the edge (its row position in the
/// `edges` table). The degree and `node_edges` paths read these maps only.
///
/// `MemoryGraph` keeps edges in insertion order and reports adjacency in that
/// order; redb orders rows by key, so `seq` is what reproduces the reference
/// ordering without a second table.
struct Adjacency {
    out: HashMap<String, Vec<(String, u64)>>,
    incoming: HashMap<String, Vec<(String, u64)>>,
}

impl Adjacency {
    fn new() -> Self {
        Self {
            out: HashMap::new(),
            incoming: HashMap::new(),
        }
    }

    fn insert(&mut self, src: &str, dst: &str, seq: u64) {
        self.out
            .entry(src.to_string())
            .or_default()
            .push((dst.to_string(), seq));
        self.incoming
            .entry(dst.to_string())
            .or_default()
            .push((src.to_string(), seq));
    }

    fn remove(&mut self, src: &str, dst: &str) {
        if let Some(rows) = self.out.get_mut(src) {
            rows.retain(|(target, _)| target != dst);
            if rows.is_empty() {
                self.out.remove(src);
            }
        }
        if let Some(rows) = self.incoming.get_mut(dst) {
            rows.retain(|(source, _)| source != src);
            if rows.is_empty() {
                self.incoming.remove(dst);
            }
        }
    }

    fn remove_node(&mut self, node_id: &str) {
        // Collect first: `remove` mutates both maps, so a borrow held across
        // the loop would fight it.
        let touching: Vec<(String, String)> = self
            .out
            .get(node_id)
            .map(|rows| {
                rows.iter()
                    .map(|(dst, _)| (node_id.to_string(), dst.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .chain(
                self.incoming
                    .get(node_id)
                    .map(|rows| {
                        rows.iter()
                            .map(|(src, _)| (src.clone(), node_id.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
            .collect();
        for (src, dst) in touching {
            self.remove(&src, &dst);
        }
    }

    fn degree(&self, node_id: &str) -> i64 {
        let out = self.out.get(node_id).map_or(0, Vec::len);
        let incoming = self.incoming.get(node_id).map_or(0, Vec::len);
        (out + incoming) as i64
    }

    /// `(queried node, other)` pairs in `MemoryGraph` order: outgoing first,
    /// then incoming, each in edge insertion order.
    fn neighbours(&self, node_id: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(rows) = self.out.get(node_id) {
            let mut ordered = rows.clone();
            ordered.sort_by_key(|(_, seq)| *seq);
            out.extend(
                ordered
                    .into_iter()
                    .map(|(dst, _)| (node_id.to_string(), dst)),
            );
        }
        if let Some(rows) = self.incoming.get(node_id) {
            let mut ordered = rows.clone();
            ordered.sort_by_key(|(_, seq)| *seq);
            out.extend(
                ordered
                    .into_iter()
                    .map(|(src, _)| (src, node_id.to_string())),
            );
        }
        out
    }
}

pub struct RedbGraph {
    db: Arc<Database>,
    scope: String,
    /// Guards both the in-memory adjacency and redb writes. A `Mutex` (not
    /// `RwLock`) because every `GraphStore` method reads or mutates it.
    state: Mutex<GraphState>,
}

struct GraphState {
    adjacency: Adjacency,
    /// Node ids in insertion order, so `snapshot` reports nodes the way
    /// `MemoryGraph` does rather than in key order.
    node_order: Vec<String>,
    /// Membership mirror of `node_order`, so `has_node` stays O(1) instead of
    /// scanning the order vector on every call.
    node_set: HashSet<String>,
    /// Next edge sequence number.
    next_seq: u64,
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
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

fn decode(raw: &str) -> Result<Json> {
    serde_json::from_str(raw).map_err(backend)
}

impl RedbGraph {
    /// Open (creating if needed) the graph file and load its adjacency index.
    ///
    /// The load is a single sequential pass over both tables, so reopen cost
    /// is linear in graph size — the price of keeping traversal in memory.
    pub async fn open(path: impl AsRef<Path>, scope: impl Into<String>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| backend(format!("creating {}: {error}", parent.display())))?;
            }
        }
        let db = Database::create(path).map_err(backend)?;
        let scope = scope.into();

        let mut adjacency = Adjacency::new();
        let mut node_order = Vec::new();
        let mut next_seq = 0u64;

        {
            let txn = db.begin_read().map_err(backend)?;
            if let Ok(nodes) = txn.open_table(NODES) {
                for entry in nodes.iter().map_err(backend)? {
                    let (key, _) = entry.map_err(backend)?;
                    let (_, id) = key.value();
                    node_order.push(id.to_string());
                }
            }
            if let Ok(edges) = txn.open_table(EDGES) {
                for entry in edges.iter().map_err(backend)? {
                    let (key, _) = entry.map_err(backend)?;
                    let (_, src, dst) = key.value();
                    adjacency.insert(src, dst, next_seq);
                    next_seq += 1;
                }
            }
        }

        Ok(Self {
            db: Arc::new(db),
            scope,
            state: Mutex::new(GraphState {
                adjacency,
                node_order: node_order.to_vec(),
                node_set: node_order.into_iter().collect(),
                next_seq,
            }),
        })
    }

    async fn read_node_row(&self, node_id: &str) -> Result<Option<Json>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = match txn.open_table(NODES) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(backend(error)),
        };
        match table.get((self.scope.as_str(), node_id)).map_err(backend)? {
            Some(guard) => Ok(Some(decode(guard.value())?)),
            None => Ok(None),
        }
    }

    async fn read_edge_row(&self, src: &str, tgt: &str) -> Result<Option<Json>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = match txn.open_table(EDGES) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(backend(error)),
        };
        match table
            .get((self.scope.as_str(), src, tgt))
            .map_err(backend)?
        {
            Some(guard) => Ok(Some(decode(guard.value())?)),
            None => Ok(None),
        }
    }

    /// Write one node, merging attributes against what is already stored.
    async fn write_node(&self, node_id: &str, data: Json) -> Result<()> {
        let existing = self.read_node_row(node_id).await?;
        let merged = merge_attributes(existing.as_ref(), &data);
        let payload = merged.to_string();
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(NODES).map_err(backend)?;
            table
                .insert((self.scope.as_str(), node_id), payload.as_str())
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        let mut state = self.state.lock().await;
        if state.node_set.insert(node_id.to_string()) {
            state.node_order.push(node_id.to_string());
        }
        Ok(())
    }

    async fn write_edge(&self, src: &str, tgt: &str, data: Json) -> Result<()> {
        let existing = self.read_edge_row(src, tgt).await?;
        let merged = merge_attributes(existing.as_ref(), &data);
        let payload = merged.to_string();
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(EDGES).map_err(backend)?;
            table
                .insert((self.scope.as_str(), src, tgt), payload.as_str())
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        let mut state = self.state.lock().await;
        let already_present = state
            .adjacency
            .out
            .get(src)
            .is_some_and(|rows| rows.iter().any(|(dst, _)| dst == tgt));
        if !already_present {
            let seq = state.next_seq;
            state.next_seq += 1;
            state.adjacency.insert(src, tgt, seq);
        }
        Ok(())
    }
}

#[async_trait]
impl GraphStore for RedbGraph {
    async fn has_node(&self, node_id: &str) -> Result<bool> {
        let state = self.state.lock().await;
        Ok(state.node_set.contains(node_id))
    }

    async fn has_edge(&self, src: &str, tgt: &str) -> Result<bool> {
        let state = self.state.lock().await;
        Ok(state
            .adjacency
            .out
            .get(src)
            .is_some_and(|rows| rows.iter().any(|(dst, _)| dst == tgt)))
    }

    async fn get_node(&self, node_id: &str) -> Result<Option<Json>> {
        self.read_node_row(node_id).await
    }

    async fn get_nodes_batch(&self, ids: &[String]) -> Result<Vec<Option<Json>>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.read_node_row(id).await?);
        }
        Ok(out)
    }

    async fn get_edge(&self, src: &str, tgt: &str) -> Result<Option<Json>> {
        self.read_edge_row(src, tgt).await
    }

    async fn get_edges_batch(&self, pairs: &[(String, String)]) -> Result<Vec<Option<Json>>> {
        let mut out = Vec::with_capacity(pairs.len());
        for (src, tgt) in pairs {
            out.push(self.read_edge_row(src, tgt).await?);
        }
        Ok(out)
    }

    async fn node_degree(&self, node_id: &str) -> Result<i64> {
        let state = self.state.lock().await;
        Ok(state.adjacency.degree(node_id))
    }

    async fn node_degrees_batch(&self, ids: &[String]) -> Result<Vec<i64>> {
        let state = self.state.lock().await;
        Ok(ids.iter().map(|id| state.adjacency.degree(id)).collect())
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
        let state = self.state.lock().await;
        if !state.node_set.contains(node_id) {
            return Ok(None);
        }
        Ok(Some(state.adjacency.neighbours(node_id)))
    }

    async fn nodes_edges_batch(
        &self,
        ids: &[String],
    ) -> Result<Vec<Option<Vec<(String, String)>>>> {
        let state = self.state.lock().await;
        Ok(ids
            .iter()
            .map(|id| {
                if state.node_set.contains(id) {
                    Some(state.adjacency.neighbours(id))
                } else {
                    None
                }
            })
            .collect())
    }

    async fn upsert_node(&self, node_id: &str, data: Json) -> Result<()> {
        self.write_node(node_id, data).await
    }

    async fn upsert_nodes_batch(&self, rows: Vec<(String, Json)>) -> Result<()> {
        for (id, data) in rows {
            self.write_node(&id, data).await?;
        }
        Ok(())
    }

    async fn upsert_edge(&self, src: &str, tgt: &str, data: Json) -> Result<()> {
        self.write_edge(src, tgt, data).await
    }

    async fn upsert_edges_batch(&self, rows: Vec<(String, String, Json)>) -> Result<()> {
        for (src, tgt, data) in rows {
            self.write_edge(&src, &tgt, data).await?;
        }
        Ok(())
    }

    async fn remove_node(&self, node_id: &str) -> Result<()> {
        let touching: Vec<(String, String)> = {
            let state = self.state.lock().await;
            state.adjacency.neighbours(node_id)
        };
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut nodes = txn.open_table(NODES).map_err(backend)?;
            nodes
                .remove((self.scope.as_str(), node_id))
                .map_err(backend)?;
        }
        {
            let mut edges = txn.open_table(EDGES).map_err(backend)?;
            for (src, dst) in &touching {
                edges
                    .remove((self.scope.as_str(), src.as_str(), dst.as_str()))
                    .map_err(backend)?;
            }
        }
        txn.commit().map_err(backend)?;
        let mut state = self.state.lock().await;
        state.node_order.retain(|id| id != node_id);
        state.adjacency.remove_node(node_id);
        Ok(())
    }

    async fn remove_edge(&self, src: &str, tgt: &str) -> Result<()> {
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut edges = txn.open_table(EDGES).map_err(backend)?;
            edges
                .remove((self.scope.as_str(), src, tgt))
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        let mut state = self.state.lock().await;
        state.adjacency.remove(src, tgt);
        Ok(())
    }

    async fn snapshot(&self) -> Result<GraphSnapshot> {
        // Copy the shape under the lock, then read the rows outside it: the
        // row readers take no lock, but holding the state lock across those
        // awaits would block every other graph call for the whole snapshot.
        let (node_ids, mut edge_pairs) = {
            let state = self.state.lock().await;
            let mut rows: Vec<(u64, String, String)> = Vec::new();
            for (src, out) in &state.adjacency.out {
                for (dst, seq) in out {
                    rows.push((*seq, src.clone(), dst.clone()));
                }
            }
            (state.node_order.clone(), rows)
        };
        let mut nodes = Vec::with_capacity(node_ids.len());
        for id in &node_ids {
            let props = self.read_node_row(id).await?.unwrap_or_default();
            nodes.push((id.clone(), props));
        }
        edge_pairs.sort_by_key(|(seq, _, _)| *seq);
        let mut edges = Vec::with_capacity(edge_pairs.len());
        for (_, src, dst) in edge_pairs {
            let props = self.read_edge_row(&src, &dst).await?.unwrap_or_default();
            edges.push((src, dst, props));
        }
        Ok(GraphSnapshot { nodes, edges })
    }

    async fn index_done(&self) -> Result<()> {
        // Writes commit per call; nothing to flush.
        Ok(())
    }
}
