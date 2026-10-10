//! In-memory backends: deterministic implementations used by tests and the
//! offline pipeline path. Semantics mirror the reference's
//! json / nano-vectordb / networkx behaviour (insertion-ordered maps) so the
//! ported pipeline can be exercised without native stores.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::core::traits::{
    Embedder, GraphSnapshot, GraphStore, KvStore, Result, VectorHit, VectorRow, VectorStore,
};

/// `JsonKVStorage` semantics (`_storage/kv_json.py`): one map, last write wins.
#[derive(Default)]
pub struct MemoryKv {
    data: Mutex<HashMap<String, Value>>,
}

impl MemoryKv {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl KvStore for MemoryKv {
    async fn all_keys(&self) -> Result<Vec<String>> {
        Ok(self.data.lock().expect("kv lock").keys().cloned().collect())
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Value>> {
        Ok(self.data.lock().expect("kv lock").get(id).cloned())
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<Option<Value>>> {
        let data = self.data.lock().expect("kv lock");
        Ok(ids.iter().map(|id| data.get(id).cloned()).collect())
    }

    async fn filter_keys(&self, ids: &[String]) -> Result<Vec<String>> {
        let data = self.data.lock().expect("kv lock");
        Ok(ids
            .iter()
            .filter(|id| !data.contains_key(*id))
            .cloned()
            .collect())
    }

    async fn upsert(&self, rows: Vec<(String, Value)>) -> Result<()> {
        let mut data = self.data.lock().expect("kv lock");
        for (key, value) in rows {
            data.insert(key, value);
        }
        Ok(())
    }

    async fn drop_all(&self) -> Result<()> {
        self.data.lock().expect("kv lock").clear();
        Ok(())
    }

    async fn remove(&self, keys: &[String]) -> Result<()> {
        let mut data = self.data.lock().expect("kv lock");
        data.retain(|key, _| !keys.contains(key));
        Ok(())
    }

    async fn index_done(&self) -> Result<()> {
        Ok(())
    }
}

/// `NanoVectorDBStorage` semantics: cosine threshold default 0.2, `__metrics__`
/// as the reported distance.
pub struct MemoryVector {
    embedder: Arc<dyn Embedder>,
    /// Rows in insertion order plus a map for lookup.
    rows: Mutex<Vec<(String, Vec<f32>, Value)>>,
    cosine_threshold: f32,
}

impl MemoryVector {
    pub fn new(embedder: Arc<dyn Embedder>, cosine_threshold: f32) -> Self {
        Self {
            embedder,
            rows: Mutex::new(Vec::new()),
            cosine_threshold,
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

#[async_trait]
impl VectorStore for MemoryVector {
    async fn upsert(&self, rows: Vec<VectorRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let texts: Vec<String> = rows.iter().map(|row| row.content.clone()).collect();
        let embeddings = self.embedder.embed(&texts).await?;
        let mut store = self.rows.lock().expect("vector lock");
        for (row, embedding) in rows.into_iter().zip(embeddings) {
            if let Some(existing) = store.iter_mut().find(|(id, _, _)| *id == row.id) {
                existing.1 = embedding;
                existing.2 = row.meta;
            } else {
                store.push((row.id, embedding, row.meta));
            }
        }
        Ok(())
    }

    async fn query(&self, query: &str, top_k: usize) -> Result<Vec<VectorHit>> {
        let embedding = self.embedder.embed(&[query.to_string()]).await?.remove(0);
        let store = self.rows.lock().expect("vector lock");
        let mut hits: Vec<VectorHit> = store
            .iter()
            .map(|(id, vector, meta)| VectorHit {
                id: id.clone(),
                distance: cosine(&embedding, vector),
                meta: meta.clone(),
            })
            .filter(|hit| hit.distance >= self.cosine_threshold)
            .collect();
        hits.sort_by(|a, b| {
            b.distance
                .partial_cmp(&a.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        Ok(hits)
    }

    async fn remove(&self, ids: &[String]) -> Result<()> {
        self.rows
            .lock()
            .expect("vector lock")
            .retain(|(id, _, _)| !ids.contains(id));
        Ok(())
    }

    async fn index_done(&self) -> Result<()> {
        Ok(())
    }
}

/// `NetworkXStorage` semantics over insertion-ordered node/edge lists.
#[derive(Default)]
pub struct MemoryGraph {
    nodes: Mutex<Vec<(String, Value)>>,
    edges: Mutex<Vec<((String, String), Value)>>,
}

impl MemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl GraphStore for MemoryGraph {
    async fn has_node(&self, node_id: &str) -> Result<bool> {
        Ok(self
            .nodes
            .lock()
            .expect("graph lock")
            .iter()
            .any(|(id, _)| id == node_id))
    }

    async fn has_edge(&self, src: &str, tgt: &str) -> Result<bool> {
        Ok(self
            .edges
            .lock()
            .expect("graph lock")
            .iter()
            .any(|((a, b), _)| a == src && b == tgt))
    }

    async fn get_node(&self, node_id: &str) -> Result<Option<Value>> {
        Ok(self
            .nodes
            .lock()
            .expect("graph lock")
            .iter()
            .find(|(id, _)| id == node_id)
            .map(|(_, data)| data.clone()))
    }

    async fn get_nodes_batch(&self, ids: &[String]) -> Result<Vec<Option<Value>>> {
        let nodes = self.nodes.lock().expect("graph lock");
        Ok(ids
            .iter()
            .map(|id| {
                nodes
                    .iter()
                    .find(|(node_id, _)| node_id == id)
                    .map(|(_, data)| data.clone())
            })
            .collect())
    }

    async fn get_edge(&self, src: &str, tgt: &str) -> Result<Option<Value>> {
        Ok(self
            .edges
            .lock()
            .expect("graph lock")
            .iter()
            .find(|((a, b), _)| a == src && b == tgt)
            .map(|(_, data)| data.clone()))
    }

    async fn get_edges_batch(&self, pairs: &[(String, String)]) -> Result<Vec<Option<Value>>> {
        let edges = self.edges.lock().expect("graph lock");
        Ok(pairs
            .iter()
            .map(|(src, tgt)| {
                edges
                    .iter()
                    .find(|((a, b), _)| a == src && b == tgt)
                    .map(|(_, data)| data.clone())
            })
            .collect())
    }

    async fn node_degree(&self, node_id: &str) -> Result<i64> {
        Ok(self
            .edges
            .lock()
            .expect("graph lock")
            .iter()
            .filter(|((a, b), _)| a == node_id || b == node_id)
            .count() as i64)
    }

    async fn node_degrees_batch(&self, ids: &[String]) -> Result<Vec<i64>> {
        let edges = self.edges.lock().expect("graph lock");
        Ok(ids
            .iter()
            .map(|id| {
                edges
                    .iter()
                    .filter(|((a, b), _)| a == id || b == id)
                    .count() as i64
            })
            .collect())
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
        let nodes = self.nodes.lock().expect("graph lock");
        if !nodes.iter().any(|(id, _)| id == node_id) {
            return Ok(None);
        }
        let edges = self.edges.lock().expect("graph lock");
        Ok(Some(
            edges
                .iter()
                .filter(|((a, b), _)| a == node_id || b == node_id)
                .map(|((a, b), _)| {
                    if a == node_id {
                        (a.clone(), b.clone())
                    } else {
                        (b.clone(), a.clone())
                    }
                })
                .collect(),
        ))
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

    async fn upsert_node(&self, node_id: &str, data: Value) -> Result<()> {
        let mut nodes = self.nodes.lock().expect("graph lock");
        match nodes.iter_mut().find(|(id, _)| id == node_id) {
            Some(existing) => {
                // networkx add_node merges attributes (last write wins per key).
                if let (Some(old), Some(new)) = (existing.1.as_object_mut(), data.as_object()) {
                    for (key, value) in new {
                        old.insert(key.clone(), value.clone());
                    }
                } else {
                    existing.1 = data;
                }
            }
            None => nodes.push((node_id.to_string(), data)),
        }
        Ok(())
    }

    async fn upsert_nodes_batch(&self, rows: Vec<(String, Value)>) -> Result<()> {
        for (id, data) in rows {
            self.upsert_node(&id, data).await?;
        }
        Ok(())
    }

    async fn upsert_edge(&self, src: &str, tgt: &str, data: Value) -> Result<()> {
        let mut edges = self.edges.lock().expect("graph lock");
        match edges.iter_mut().find(|((a, b), _)| a == src && b == tgt) {
            Some(existing) => {
                if let (Some(old), Some(new)) = (existing.1.as_object_mut(), data.as_object()) {
                    for (key, value) in new {
                        old.insert(key.clone(), value.clone());
                    }
                } else {
                    existing.1 = data;
                }
            }
            None => edges.push(((src.to_string(), tgt.to_string()), data)),
        }
        Ok(())
    }

    async fn upsert_edges_batch(&self, rows: Vec<(String, String, Value)>) -> Result<()> {
        for (src, tgt, data) in rows {
            self.upsert_edge(&src, &tgt, data).await?;
        }
        Ok(())
    }

    async fn remove_node(&self, node_id: &str) -> Result<()> {
        self.nodes
            .lock()
            .expect("graph lock")
            .retain(|(id, _)| id != node_id);
        self.edges
            .lock()
            .expect("graph lock")
            .retain(|((src, tgt), _)| src != node_id && tgt != node_id);
        Ok(())
    }

    async fn remove_edge(&self, src: &str, tgt: &str) -> Result<()> {
        self.edges
            .lock()
            .expect("graph lock")
            .retain(|((existing_src, existing_tgt), _)| {
                !(existing_src == src && existing_tgt == tgt)
            });
        Ok(())
    }

    async fn snapshot(&self) -> Result<GraphSnapshot> {
        Ok(GraphSnapshot {
            nodes: self.nodes.lock().expect("graph lock").clone(),
            edges: self
                .edges
                .lock()
                .expect("graph lock")
                .iter()
                .map(|((a, b), data)| (a.clone(), b.clone(), data.clone()))
                .collect(),
        })
    }

    async fn index_done(&self) -> Result<()> {
        Ok(())
    }
}
