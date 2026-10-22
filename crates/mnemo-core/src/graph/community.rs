//! Community detection — port of `gdb_networkx.py::clustering` (165-168,
//! 230-252) and `community_schema` (170-228).
//!
//! Deviation (documented in the stage doc): graspologic's
//! `hierarchical_leiden` → `leiden-rs` `run_hierarchical`. Same family of
//! algorithm, different implementation — partition equality is not claimed;
//! the schema shape and the invariants (levels, coverage, determinism under a
//! fixed seed) are.
//!
//! Cluster ids: the reference relies on graspologic numbering communities
//! globally across levels. `leiden-rs` numbers communities per level, so we
//! assign consecutive ids in level-then-node order — deterministic and unique.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use leiden_rs::{GraphDataBuilder, Leiden, LeidenConfig};
use serde_json::{json, Value};

use crate::core::rag::CommunitySchema;
use crate::core::text::GRAPH_FIELD_SEP;
use crate::core::traits::{GraphSnapshot, GraphStore, Result, StoreError};

/// `max_graph_cluster_size` default (`graphrag.py:88`).
pub const DEFAULT_MAX_GRAPH_CLUSTER_SIZE: usize = 10;
/// `graph_cluster_seed` default (`graphrag.py:89`).
pub const DEFAULT_GRAPH_CLUSTER_SEED: u64 = 0xDEADBEEF;

#[derive(Debug, Clone)]
pub struct CommunityOptions {
    pub max_cluster_size: usize,
    pub seed: u64,
}

impl Default for CommunityOptions {
    fn default() -> Self {
        Self {
            max_cluster_size: DEFAULT_MAX_GRAPH_CLUSTER_SIZE,
            seed: DEFAULT_GRAPH_CLUSTER_SEED,
        }
    }
}

/// One node's cluster membership at a level
/// (`{"level": l, "cluster": c}`, `gdb_networkx.py:243-244`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClusterRef {
    pub level: i64,
    pub cluster: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CommunityResult {
    /// Node id -> clusters, only for nodes inside the largest connected
    /// component (`_cluster_data_to_subgraphs`, gdb_networkx.py:226-228).
    pub memberships: Vec<(String, Vec<ClusterRef>)>,
    /// Schema rows in cluster-id order.
    pub schema: Vec<(i64, CommunitySchema)>,
}

/// Detect communities and write the `clusters` attribute back to the graph.
pub async fn detect_communities(
    graph: &dyn GraphStore,
    options: &CommunityOptions,
) -> Result<CommunityResult> {
    let snapshot = graph.snapshot().await?;
    let result = detect_communities_from_snapshot(&snapshot, options)?;
    for (node, clusters) in &result.memberships {
        let mut data = graph.get_node(node).await?.unwrap_or_else(|| json!({}));
        data["clusters"] =
            serde_json::to_value(clusters).map_err(|e| StoreError::Backend(e.to_string()))?;
        graph.upsert_node(node, data).await?;
    }
    Ok(result)
}

/// Pure computation over a snapshot (no store writes) — used by tests.
pub fn detect_communities_from_snapshot(
    snapshot: &GraphSnapshot,
    options: &CommunityOptions,
) -> Result<CommunityResult> {
    let names: Vec<String> = snapshot.nodes.iter().map(|(id, _)| id.clone()).collect();
    let index_of: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();

    // Undirected adjacency (duplicate pairs collapse; last write wins, like
    // networkx attribute assignment).
    let mut weights: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for (src, tgt, data) in &snapshot.edges {
        let (Some(&a), Some(&b)) = (index_of.get(src.as_str()), index_of.get(tgt.as_str())) else {
            continue;
        };
        if a == b {
            continue;
        }
        let weight = data.get("weight").and_then(Value::as_f64).unwrap_or(1.0);
        let key = if a < b { (a, b) } else { (b, a) };
        weights.insert(key, weight);
    }
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    for &(a, b) in weights.keys() {
        adjacency[a].push(b);
        adjacency[b].push(a);
    }

    let lcc = largest_connected_component(&adjacency);
    if lcc.is_empty() {
        return Ok(CommunityResult::default());
    }
    let mut lcc_index: HashMap<usize, usize> = HashMap::new();
    for (position, &node) in lcc.iter().enumerate() {
        lcc_index.insert(node, position);
    }

    let mut builder = GraphDataBuilder::new(lcc.len());
    for (&(a, b), &weight) in &weights {
        let (Some(&i), Some(&j)) = (lcc_index.get(&a), lcc_index.get(&b)) else {
            continue;
        };
        builder
            .add_edge(i, j, weight)
            .map_err(|e| StoreError::Backend(format!("leiden edge: {e}")))?;
    }
    let data = builder
        .build()
        .map_err(|e| StoreError::Backend(format!("leiden graph: {e}")))?;
    let config = LeidenConfig::builder()
        .max_comm_size(options.max_cluster_size)
        .maybe_seed(Some(options.seed))
        .build();
    let hierarchy = Leiden::new(config)
        .run_hierarchical(&data)
        .map_err(|e| StoreError::Backend(format!("leiden run: {e}")))?;

    // Node -> clusters, with globally unique cluster ids assigned in
    // level-then-node order.
    let mut memberships: Vec<(String, Vec<ClusterRef>)> = lcc
        .iter()
        .map(|&index| (names[index].clone(), Vec::new()))
        .collect();
    let mut assigned: HashMap<(usize, usize), i64> = HashMap::new();
    let mut next_cluster_id = 0i64;
    for (level_index, level) in hierarchy.levels.iter().enumerate() {
        for (position, _) in lcc.iter().enumerate() {
            let raw = level.membership[position];
            let cluster = *assigned.entry((level_index, raw)).or_insert_with(|| {
                let id = next_cluster_id;
                next_cluster_id += 1;
                id
            });
            memberships[position].1.push(ClusterRef {
                level: level_index as i64,
                cluster,
            });
        }
    }

    let schema = build_schema_from_snapshot(snapshot, &memberships);
    Ok(CommunityResult {
        memberships,
        schema,
    })
}

/// Largest connected component, deterministically: most nodes wins; ties go to
/// the component whose smallest node index is smaller.
fn largest_connected_component(adjacency: &[Vec<usize>]) -> Vec<usize> {
    let mut visited = vec![false; adjacency.len()];
    let mut best: Vec<usize> = Vec::new();
    for start in 0..adjacency.len() {
        if visited[start] {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        visited[start] = true;
        while let Some(node) = queue.pop_front() {
            component.push(node);
            for &neighbor in &adjacency[node] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        component.sort_unstable();
        if component.len() > best.len() {
            best = component;
        }
    }
    best
}

/// `community_schema` (`gdb_networkx.py:170-228`) over a snapshot plus known
/// memberships. Public so the query paths can rebuild the schema without
/// re-running clustering.
pub fn build_schema_from_snapshot(
    snapshot: &GraphSnapshot,
    memberships: &[(String, Vec<ClusterRef>)],
) -> Vec<(i64, CommunitySchema)> {
    let node_data: HashMap<&str, &Value> = snapshot
        .nodes
        .iter()
        .map(|(id, data)| (id.as_str(), data))
        .collect();

    // Edges incident to each node, as sorted pairs (reference uses
    // `this_node_edges` from the original graph).
    let mut incident: HashMap<&str, BTreeSet<(String, String)>> = HashMap::new();
    for (src, tgt, _) in &snapshot.edges {
        let pair = if src <= tgt {
            (src.clone(), tgt.clone())
        } else {
            (tgt.clone(), src.clone())
        };
        incident
            .entry(src.as_str())
            .or_default()
            .insert(pair.clone());
        incident.entry(tgt.as_str()).or_default().insert(pair);
    }

    let mut rows: Vec<(i64, CommunitySchema)> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    let mut levels: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    let mut max_num_ids = 0usize;

    for (name, clusters) in memberships {
        let chunk_ids: Vec<String> = node_data
            .get(name.as_str())
            .and_then(|data| data.get("source_id"))
            .and_then(Value::as_str)
            .map(|value| {
                value
                    .split(GRAPH_FIELD_SEP)
                    .filter(|part| !part.is_empty())
                    .map(|part| part.to_string())
                    .collect()
            })
            .unwrap_or_default();
        for cluster in clusters {
            let position = *index.entry(cluster.cluster).or_insert_with(|| {
                rows.push((
                    cluster.cluster,
                    CommunitySchema {
                        level: cluster.level,
                        title: format!("Cluster {}", cluster.cluster),
                        edges: Vec::new(),
                        nodes: Vec::new(),
                        chunk_ids: Vec::new(),
                        occurrence: 0.0,
                        sub_communities: Vec::new(),
                        report_string: None,
                        report_json: None,
                    },
                ));
                rows.len() - 1
            });
            let row = &mut rows[position].1;
            row.level = cluster.level;
            row.nodes.push(name.clone());
            if let Some(edges) = incident.get(name.as_str()) {
                row.edges.extend(edges.iter().cloned());
            }
            row.chunk_ids.extend(chunk_ids.iter().cloned());
            levels
                .entry(cluster.level)
                .or_default()
                .insert(cluster.cluster);
            max_num_ids = max_num_ids.max(row.chunk_ids.len());
        }
    }

    // Sets in the reference: dedupe and sort for reproducibility.
    for (_, row) in rows.iter_mut() {
        row.nodes.sort();
        row.nodes.dedup();
        row.edges.sort();
        row.edges.dedup();
        row.chunk_ids.sort();
        row.chunk_ids.dedup();
        row.occurrence = if max_num_ids == 0 {
            0.0
        } else {
            row.chunk_ids.len() as f64 / max_num_ids as f64
        };
    }

    // Sub-communities: adjacent levels, node-subset test.
    let ordered_levels: Vec<i64> = levels.keys().copied().collect();
    for window in ordered_levels.windows(2) {
        let (this_level, next_level) = (window[0], window[1]);
        let this_communities: Vec<i64> = levels[&this_level].iter().copied().collect();
        let next_communities: Vec<i64> = levels[&next_level].iter().copied().collect();
        for comm in this_communities {
            let position = index[&comm];
            let nodes: BTreeSet<String> = rows[position].1.nodes.iter().cloned().collect();
            let subs: Vec<String> = next_communities
                .iter()
                .filter(|child| {
                    let child_position = index[*child];
                    rows[child_position]
                        .1
                        .nodes
                        .iter()
                        .all(|node| nodes.contains(node))
                })
                .map(|child| child.to_string())
                .collect();
            rows[position].1.sub_communities = subs;
        }
    }

    rows.sort_by_key(|(cluster, _)| *cluster);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_cliques() -> GraphSnapshot {
        let mut nodes = Vec::new();
        for name in ["A1", "A2", "A3", "A4", "B1", "B2", "B3", "B4", "ISOLATED"] {
            nodes.push((
                name.to_string(),
                json!({"source_id": format!("chunk-{name}")}),
            ));
        }
        let mut edges = Vec::new();
        let clique_a = ["A1", "A2", "A3", "A4"];
        let clique_b = ["B1", "B2", "B3", "B4"];
        for (clique, prefix) in [(&clique_a, "a"), (&clique_b, "b")] {
            for i in 0..clique.len() {
                for j in (i + 1)..clique.len() {
                    edges.push((
                        clique[i].to_string(),
                        clique[j].to_string(),
                        json!({"weight": 5.0, "description": format!("{prefix}-{i}-{j}")}),
                    ));
                }
            }
        }
        edges.push((
            "A1".to_string(),
            "B1".to_string(),
            json!({"weight": 1.0, "description": "bridge"}),
        ));
        GraphSnapshot { nodes, edges }
    }

    #[test]
    fn detects_levels_and_covers_the_component() {
        let snapshot = snapshot_with_cliques();
        let result =
            detect_communities_from_snapshot(&snapshot, &CommunityOptions::default()).expect("run");

        assert_eq!(
            result.memberships.len(),
            8,
            "the isolated node is not clustered"
        );
        assert!(
            result
                .memberships
                .iter()
                .all(|(_, clusters)| !clusters.is_empty()),
            "every clustered node has at least one level"
        );
        let levels: BTreeSet<i64> = result
            .memberships
            .iter()
            .flat_map(|(_, clusters)| clusters.iter().map(|c| c.level))
            .collect();
        assert!(!levels.is_empty());

        assert!(!result.schema.is_empty());
        for (_, row) in &result.schema {
            assert!(
                row.occurrence > 0.0 && row.occurrence <= 1.0,
                "occurrence in range"
            );
            assert!(!row.nodes.is_empty());
            assert!(row.title.starts_with("Cluster "));
        }
    }

    #[test]
    fn same_seed_is_deterministic() {
        let snapshot = snapshot_with_cliques();
        let first = detect_communities_from_snapshot(&snapshot, &CommunityOptions::default())
            .expect("first");
        let second = detect_communities_from_snapshot(&snapshot, &CommunityOptions::default())
            .expect("second");
        assert_eq!(first.memberships, second.memberships);
        assert_eq!(first.schema.len(), second.schema.len());
    }
}
