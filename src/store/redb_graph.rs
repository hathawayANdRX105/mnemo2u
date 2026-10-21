//! Redb graph store — the durable graph arm.
//!
//! Two tables in one redb file, keyed so a scope never mixes rows:
//! `nodes(scope, id) -> props_json` and `edges(scope, src, dst) -> props_json`.
//!
//! Why redb rather than turso adjacency tables or a graph engine:
//! turso holds a single `Connection` behind a `tokio::Mutex`, so a graph call
//! used to serialise against the KV/fact writes on the *same* connection. A
//! separate redb file gives the graph its own lock, and the whole graph lives
//! in memory — traversal, degrees and adjacency never touch the disk. kuzu was
//! the other candidate and is out for good: the upstream repository is
//! archived and its static link collides with `turso_core`'s vendored simsimd
//! (`todo/01-nano-graphrag-port.md` §8.5 records the link evidence; §8.7
//! records this switch).
//!
//! Semantics are `MemoryGraph`'s (`BaseGraphStorage`, base.py:117-186);
//! `tests/redb_graph.rs` pins that contract.
//!
//! Derivative: the file can be dropped and rebuilt from the truth layer —
//! the same "rebuildable derived index" rule the vector index follows.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};

use async_trait::async_trait;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde_json::Value as Json;
use tokio::sync::RwLock;

use crate::core::traits::{GraphSnapshot, GraphStore, Result, StoreError};

const NODES: TableDefinition<(&str, &str), &str> = TableDefinition::new("graph_node");
const EDGES: TableDefinition<(&str, &str, &str), &str> = TableDefinition::new("graph_edge");

/// Live graph files, keyed by path. The entry is a `Weak`: when the last
/// handle to a file goes away the `Database` drops, the file lock is released
/// and the next `open` loads the file from disk again — a real restart.
/// While a handle is alive, a second `open` of the same path shares it
/// instead of failing on redb's exclusive file lock.
static REGISTRY: LazyLock<StdMutex<HashMap<String, Weak<SharedGraph>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// In-memory adjacency for one scope: `src -> [(dst, seq)]` and
/// `dst -> [(src, seq)]`, where `seq` is the edge's insertion order.
///
/// `MemoryGraph` keeps edges in insertion order and reports adjacency in that
/// order; redb orders rows by key, so `seq` is what reproduces the reference
/// ordering without a second table.
#[derive(Clone, Default, PartialEq)]
struct Adjacency {
    out: HashMap<String, Vec<(String, u64)>>,
    incoming: HashMap<String, Vec<(String, u64)>>,
}

impl Adjacency {
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
    /// then incoming, each in edge insertion order. The queried node always
    /// comes first, whichever direction the edge runs.
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
                    .map(|(src, _)| (node_id.to_string(), src)),
            );
        }
        out
    }
}

/// One scope's graph, in memory.
#[derive(Clone, Default, PartialEq)]
struct ScopeIndex {
    /// Node id → attributes.
    nodes: HashMap<String, Json>,
    /// Node ids in insertion order, so `snapshot` reports nodes the way
    /// `MemoryGraph` does rather than in key order.
    node_order: Vec<String>,
    /// `(src, dst)` → attributes.
    edges: HashMap<(String, String), Json>,
    adjacency: Adjacency,
    /// Next edge sequence number for this scope.
    next_seq: u64,
}

impl ScopeIndex {
    fn note_node(&mut self, node_id: &str) {
        if !self.nodes.contains_key(node_id) {
            self.node_order.push(node_id.to_string());
        }
    }
}

/// The file-write half of a scope change, boxed so it can be awaited without
/// the compiler having to prove a `Send` bound for a generic closure.
///
/// The future borrows the candidate index it is writing, so it carries that
/// borrow's lifetime rather than being `'static`.
type BoxedWrite<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// The state of one graph file, shared by every handle on it.
struct SharedGraph {
    db: Arc<Database>,
    /// One index per scope: the row keys are scoped, so the in-memory view has
    /// to be scoped too.
    ///
    /// The map is held behind an `Arc` so a reader can take a snapshot of the
    /// pointer and read it without holding a lock: a read costs one atomic
    /// increment, and a writer replacing the map never waits for a reader
    /// that is already inside the old one.
    scopes: RwLock<Arc<HashMap<String, Arc<ScopeIndex>>>>,
}

pub struct RedbGraph {
    shared: Arc<SharedGraph>,
    scope: String,
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

/// Read every row of the file into per-scope in-memory indexes. One
/// sequential pass over both tables; reopen cost is linear in graph size.
fn load_state(db: &Database) -> Result<HashMap<String, Arc<ScopeIndex>>> {
    let mut scopes: HashMap<String, ScopeIndex> = HashMap::new();
    let txn = db.begin_read().map_err(backend)?;

    if let Ok(nodes) = txn.open_table(NODES) {
        for entry in nodes.iter().map_err(backend)? {
            let (key, value) = entry.map_err(backend)?;
            let (scope, id) = key.value();
            let index = scopes.entry(scope.to_string()).or_default();
            index.note_node(id);
            index.nodes.insert(id.to_string(), decode(value.value())?);
        }
    }

    if let Ok(edges) = txn.open_table(EDGES) {
        for entry in edges.iter().map_err(backend)? {
            let (key, value) = entry.map_err(backend)?;
            let (scope, src, dst) = key.value();
            let index = scopes.entry(scope.to_string()).or_default();
            let seq = index.next_seq;
            index.next_seq += 1;
            index.adjacency.insert(src, dst, seq);
            index
                .edges
                .insert((src.to_string(), dst.to_string()), decode(value.value())?);
        }
    }

    Ok(scopes
        .into_iter()
        .map(|(scope, index)| (scope, Arc::new(index)))
        .collect())
}

impl RedbGraph {
    /// A reader's view of the scopes: one cheap `Arc` clone, then no lock at
    /// all.
    ///
    /// A writer replacing the map does not wait for a reader that is already
    /// inside the old one — the reader keeps its snapshot until it is done,
    /// and sees the new one on its next call.
    async fn scopes(&self) -> Arc<HashMap<String, Arc<ScopeIndex>>> {
        self.shared.scopes.read().await.clone()
    }

    /// This handle's scope index, or an empty view when the scope has no rows
    /// yet.
    fn scope_index(scopes: &Arc<HashMap<String, Arc<ScopeIndex>>>, scope: &str) -> Arc<ScopeIndex> {
        scopes
            .get(scope)
            .cloned()
            .unwrap_or_else(|| Arc::new(ScopeIndex::default()))
    }

    /// Apply a change to one scope's index, with the file write outside the
    /// lock.
    ///
    /// The write is a three-step affair, and the order matters:
    ///
    /// 1. under the write lock, compute the new index from the current one
    ///    (the merge is a read-modify-write, so it needs the current value);
    /// 2. **release the lock**, then write the row to redb — that write may
    ///    block, because redb allows one write transaction at a time, and a
    ///    reader must not wait for it;
    /// 3. take the lock again and swap the new index in — but only if nobody
    ///    else changed the scope meanwhile. If they did, the row on disk now
    ///    disagrees with the index we computed from, so the change is
    ///    recomputed against the newer index and written again.
    ///
    /// The conflict policy is therefore *last write wins per row*: the
    /// recompute merges the incoming attributes onto whatever the newer
    /// index holds, so two writers touching different keys of the same node
    /// both land, and two writers touching the same key end with the one
    /// that committed last.
    async fn write_scope(
        &self,
        mutate: impl Fn(&mut ScopeIndex),
        persist: impl for<'a> Fn(&'a ScopeIndex) -> BoxedWrite<'a>,
    ) -> Result<()> {
        for _ in 0..8 {
            let current = {
                let scopes = self.shared.scopes.read().await;
                Self::scope_index(&*scopes, &self.scope)
            };
            let mut candidate = (*current).clone();
            mutate(&mut candidate);
            if candidate == *current {
                // Nothing changed: no row to write, no pointer to swap.
                return Ok(());
            }

            // The file write is outside every lock: redb serialises writers,
            // and a reader must not queue behind one.
            persist(&candidate).await?;

            let mut scopes = self.shared.scopes.write().await;
            let latest = Self::scope_index(&*scopes, &self.scope);
            if latest == current {
                // The new map is a plain `HashMap` — the `Arc` wraps the whole
                // map, not each scope.
                let mut next: HashMap<String, Arc<ScopeIndex>> = (**scopes).clone();
                next.insert(self.scope.clone(), Arc::new(candidate));
                *scopes = Arc::new(next);
                return Ok(());
            }
            // Somebody else changed the scope while the row was being
            // written: recompute against their version and write again.
            drop(scopes);
        }
        Err(StoreError::Backend(format!(
            "图写入反复冲突，未能落盘: scope={}",
            self.scope
        )))
    }

    /// Open (creating if needed) the graph file and load its in-memory index.
    ///
    /// redb holds an exclusive file lock for as long as the `Database` lives,
    /// so a second `open` of the same path in the same process would fail
    /// with `DatabaseAlreadyOpen`. The registry hands back the live instance
    /// instead, so several handles can share one file the way the turso graph
    /// did. When the last handle drops, the `Database` drops with it, the file
    /// lock is released, and a later `open` loads the file from disk again —
    /// a restart still reads what was committed, not a stale cache.
    pub async fn open(path: impl AsRef<Path>, scope: impl Into<String>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| backend(format!("creating {}: {error}", parent.display())))?;
            }
        }
        let key = path.to_string_lossy().into_owned();

        let shared = {
            let registry = REGISTRY.lock().expect("graph registry lock");
            registry.get(&key).and_then(Weak::upgrade)
        };
        let shared = match shared {
            Some(shared) => shared,
            None => {
                let db = Arc::new(Database::create(path).map_err(backend)?);
                let scopes = load_state(&db)?;
                let shared = Arc::new(SharedGraph {
                    db,
                    scopes: RwLock::new(Arc::new(scopes)),
                });
                REGISTRY
                    .lock()
                    .expect("graph registry lock")
                    .insert(key, Arc::downgrade(&shared));
                shared
            }
        };

        Ok(Self {
            shared,
            scope: scope.into(),
        })
    }
}

#[async_trait]
impl GraphStore for RedbGraph {
    async fn has_node(&self, node_id: &str) -> Result<bool> {
        let scopes = self.scopes().await;
        Ok(Self::scope_index(&scopes, &self.scope)
            .nodes
            .contains_key(node_id))
    }

    async fn has_edge(&self, src: &str, tgt: &str) -> Result<bool> {
        let scopes = self.scopes().await;
        Ok(Self::scope_index(&scopes, &self.scope)
            .edges
            .contains_key(&(src.to_string(), tgt.to_string())))
    }

    async fn get_node(&self, node_id: &str) -> Result<Option<Json>> {
        let scopes = self.scopes().await;
        Ok(Self::scope_index(&scopes, &self.scope)
            .nodes
            .get(node_id)
            .cloned())
    }

    async fn get_nodes_batch(&self, ids: &[String]) -> Result<Vec<Option<Json>>> {
        let scopes = self.scopes().await;
        let index = Self::scope_index(&scopes, &self.scope);
        Ok(ids.iter().map(|id| index.nodes.get(id).cloned()).collect())
    }

    async fn get_edge(&self, src: &str, tgt: &str) -> Result<Option<Json>> {
        let scopes = self.scopes().await;
        Ok(Self::scope_index(&scopes, &self.scope)
            .edges
            .get(&(src.to_string(), tgt.to_string()))
            .cloned())
    }

    async fn get_edges_batch(&self, pairs: &[(String, String)]) -> Result<Vec<Option<Json>>> {
        let scopes = self.scopes().await;
        let index = Self::scope_index(&scopes, &self.scope);
        Ok(pairs
            .iter()
            .map(|(src, tgt)| index.edges.get(&(src.clone(), tgt.clone())).cloned())
            .collect())
    }

    async fn node_degree(&self, node_id: &str) -> Result<i64> {
        let scopes = self.scopes().await;
        Ok(Self::scope_index(&scopes, &self.scope)
            .adjacency
            .degree(node_id))
    }

    async fn node_degrees_batch(&self, ids: &[String]) -> Result<Vec<i64>> {
        let scopes = self.scopes().await;
        let index = Self::scope_index(&scopes, &self.scope);
        Ok(ids.iter().map(|id| index.adjacency.degree(id)).collect())
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
        let scopes = self.scopes().await;
        let index = Self::scope_index(&scopes, &self.scope);
        if !index.nodes.contains_key(node_id) {
            return Ok(None);
        }
        Ok(Some(index.adjacency.neighbours(node_id)))
    }

    async fn nodes_edges_batch(
        &self,
        ids: &[String],
    ) -> Result<Vec<Option<Vec<(String, String)>>>> {
        let scopes = self.scopes().await;
        let index = Self::scope_index(&scopes, &self.scope);
        Ok(ids
            .iter()
            .map(|id| {
                if index.nodes.contains_key(id) {
                    Some(index.adjacency.neighbours(id))
                } else {
                    None
                }
            })
            .collect())
    }

    async fn upsert_node(&self, node_id: &str, data: Json) -> Result<()> {
        let scope = self.scope.clone();
        let node_id = node_id.to_string();
        self.write_scope(
            |index| {
                let merged = merge_attributes(index.nodes.get(&node_id), &data);
                index.note_node(&node_id);
                index.nodes.insert(node_id.clone(), merged);
            },
            |candidate| {
                // The closure is called more than once (a conflict sends it
                // round again), so it borrows what it needs rather than
                // moving it out.
                let scope = scope.clone();
                let node_id = node_id.clone();
                Box::pin(async move {
                    let payload = candidate
                        .nodes
                        .get(&node_id)
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    let txn = self.shared.db.begin_write().map_err(backend)?;
                    {
                        let mut table = txn.open_table(NODES).map_err(backend)?;
                        table
                            .insert((scope.as_str(), node_id.as_str()), payload.as_str())
                            .map_err(backend)?;
                    }
                    txn.commit().map_err(backend)
                })
            },
        )
        .await
    }

    async fn upsert_nodes_batch(&self, rows: Vec<(String, Json)>) -> Result<()> {
        for (id, data) in rows {
            self.upsert_node(&id, data).await?;
        }
        Ok(())
    }

    async fn upsert_edge(&self, src: &str, tgt: &str, data: Json) -> Result<()> {
        let scope = self.scope.clone();
        let src = src.to_string();
        let tgt = tgt.to_string();
        self.write_scope(
            |index| {
                let key = (src.clone(), tgt.clone());
                let merged = merge_attributes(index.edges.get(&key), &data);
                if !index.edges.contains_key(&key) {
                    let seq = index.next_seq;
                    index.next_seq += 1;
                    index.adjacency.insert(&src, &tgt, seq);
                }
                index.edges.insert(key, merged);
            },
            |candidate| {
                let scope = scope.clone();
                let src = src.clone();
                let tgt = tgt.clone();
                Box::pin(async move {
                    let payload = candidate
                        .edges
                        .get(&(src.clone(), tgt.clone()))
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    let txn = self.shared.db.begin_write().map_err(backend)?;
                    {
                        let mut table = txn.open_table(EDGES).map_err(backend)?;
                        table
                            .insert(
                                (scope.as_str(), src.as_str(), tgt.as_str()),
                                payload.as_str(),
                            )
                            .map_err(backend)?;
                    }
                    txn.commit().map_err(backend)
                })
            },
        )
        .await
    }

    async fn upsert_edges_batch(&self, rows: Vec<(String, String, Json)>) -> Result<()> {
        for (src, tgt, data) in rows {
            self.upsert_edge(&src, &tgt, data).await?;
        }
        Ok(())
    }

    async fn remove_node(&self, node_id: &str) -> Result<()> {
        let scope = self.scope.clone();
        let node_id = node_id.to_string();
        self.write_scope(
            |index| {
                let touching = index.adjacency.neighbours(&node_id);
                index.nodes.remove(&node_id);
                index.node_order.retain(|id| id != &node_id);
                for (src, dst) in touching {
                    index.edges.remove(&(src, dst));
                }
                index.adjacency.remove_node(&node_id);
            },
            |_candidate| {
                let scope = scope.clone();
                let node_id = node_id.clone();
                Box::pin(async move {
                    // The rows to delete are the ones the *current* index
                    // knows about, so they are read here rather than from
                    // the candidate: a concurrent write may have added an
                    // edge this removal did not see, and the next attempt
                    // re-reads.
                    let current = {
                        let scopes = self.shared.scopes.read().await;
                        Self::scope_index(&*scopes, &self.scope)
                    };
                    let touching = current.adjacency.neighbours(&node_id);
                    let txn = self.shared.db.begin_write().map_err(backend)?;
                    {
                        let mut nodes = txn.open_table(NODES).map_err(backend)?;
                        nodes
                            .remove((scope.as_str(), node_id.as_str()))
                            .map_err(backend)?;
                    }
                    {
                        let mut edges = txn.open_table(EDGES).map_err(backend)?;
                        for (src, dst) in &touching {
                            edges
                                .remove((scope.as_str(), src.as_str(), dst.as_str()))
                                .map_err(backend)?;
                        }
                    }
                    txn.commit().map_err(backend)
                })
            },
        )
        .await
    }
    async fn remove_edge(&self, src: &str, tgt: &str) -> Result<()> {
        let scope = self.scope.clone();
        let src = src.to_string();
        let tgt = tgt.to_string();
        self.write_scope(
            |index| {
                index.edges.remove(&(src.clone(), tgt.clone()));
                index.adjacency.remove(&src, &tgt);
            },
            |_candidate| {
                let scope = scope.clone();
                let src = src.clone();
                let tgt = tgt.clone();
                Box::pin(async move {
                    let txn = self.shared.db.begin_write().map_err(backend)?;
                    {
                        let mut edges = txn.open_table(EDGES).map_err(backend)?;
                        edges
                            .remove((scope.as_str(), src.as_str(), tgt.as_str()))
                            .map_err(backend)?;
                    }
                    txn.commit().map_err(backend)
                })
            },
        )
        .await
    }
    async fn snapshot(&self) -> Result<GraphSnapshot> {
        // The snapshot is handed to community detection, so it reads
        // everything: it takes the `Arc` once and then copies out of it,
        // rather than holding a lock across the copy.
        let scopes = self.scopes().await;
        let index = Self::scope_index(&scopes, &self.scope);
        let nodes = index
            .node_order
            .iter()
            .map(|id| (id.clone(), index.nodes.get(id).cloned().unwrap_or_default()))
            .collect();
        let mut ordered: Vec<(u64, String, String)> = Vec::with_capacity(index.edges.len());
        for (src, out) in &index.adjacency.out {
            for (dst, seq) in out {
                ordered.push((*seq, src.clone(), dst.clone()));
            }
        }
        ordered.sort_by_key(|(seq, _, _)| *seq);
        let edges = ordered
            .into_iter()
            .map(|(_, src, dst)| {
                let props = index
                    .edges
                    .get(&(src.clone(), dst.clone()))
                    .cloned()
                    .unwrap_or_default();
                (src, dst, props)
            })
            .collect();
        Ok(GraphSnapshot { nodes, edges })
    }

    async fn index_done(&self) -> Result<()> {
        // Writes commit per call; nothing to flush.
        Ok(())
    }
}
