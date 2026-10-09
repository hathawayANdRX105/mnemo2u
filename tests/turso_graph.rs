//! Turso graph contract: attribute merge, edges, degrees, adjacency ordering,
//! snapshot, scope isolation, persistence across reopen.

use mnemo2u::core::traits::GraphStore;
use mnemo2u::store::turso_graph::TursoGraph;
use serde_json::json;

#[tokio::test]
async fn graph_semantics_and_persistence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join("graph.db")
        .to_str()
        .expect("utf8 path")
        .to_string();

    {
        let graph = TursoGraph::open(&path, "entity_graph").await.expect("open");

        assert!(
            !graph.has_node("ACME").await.expect("has"),
            "fresh graph is empty"
        );
        assert!(graph.get_node("ACME").await.expect("get").is_none());
        assert!(graph.node_edges("ACME").await.expect("edges").is_none());
        assert_eq!(graph.node_degree("ACME").await.expect("degree"), 0);

        graph
            .upsert_node(
                "ACME",
                json!({"entity_type": "\"ORG\"", "source_id": "chunk-1"}),
            )
            .await
            .expect("upsert node");
        graph
            .upsert_nodes_batch(vec![
                ("BETA".to_string(), json!({"entity_type": "\"ORG\""})),
                ("GAMMA".to_string(), json!({"entity_type": "\"PERSON\""})),
            ])
            .await
            .expect("batch nodes");

        assert!(graph.has_node("ACME").await.expect("has"));
        assert_eq!(
            graph.get_node("ACME").await.expect("get").expect("row"),
            json!({"entity_type": "\"ORG\"", "source_id": "chunk-1"})
        );
        // A missing id comes back as `None`, and the batch preserves order.
        let batch = graph
            .get_nodes_batch(&["BETA".to_string(), "NOPE".to_string()])
            .await
            .expect("batch get");
        assert!(batch[0].is_some());
        assert!(batch[1].is_none());

        // A second write merges attributes key-by-key instead of replacing.
        graph
            .upsert_node(
                "ACME",
                json!({"source_id": "chunk-1<SEP>chunk-2", "clusters": [0]}),
            )
            .await
            .expect("merge node");
        let merged = graph.get_node("ACME").await.expect("get").expect("row");
        assert_eq!(
            merged["entity_type"], "\"ORG\"",
            "untouched keys survive the merge"
        );
        assert_eq!(merged["clusters"], json!([0]));

        graph
            .upsert_edge("ACME", "BETA", json!({"weight": 3.0, "order": 1}))
            .await
            .expect("upsert edge");
        assert!(graph.has_edge("ACME", "BETA").await.expect("has edge"));
        assert!(!graph
            .has_edge("BETA", "ACME")
            .await
            .expect("has edge reversed"));
        assert_eq!(
            graph
                .get_edge("ACME", "BETA")
                .await
                .expect("get edge")
                .expect("row")["weight"],
            3.0
        );

        assert_eq!(graph.node_degree("ACME").await.expect("degree"), 1);
        assert_eq!(graph.node_degree("GAMMA").await.expect("degree"), 0);
        assert_eq!(
            graph
                .edge_degree("ACME", "BETA")
                .await
                .expect("edge degree"),
            2
        );
        assert_eq!(
            graph
                .node_edges("ACME")
                .await
                .expect("edges")
                .expect("some"),
            vec![("ACME".to_string(), "BETA".to_string())]
        );
        // The queried node comes first for incoming edges too (`MemoryGraph`).
        assert_eq!(
            graph
                .node_edges("BETA")
                .await
                .expect("edges")
                .expect("some"),
            vec![("BETA".to_string(), "ACME".to_string())]
        );
        assert_eq!(
            graph
                .node_degrees_batch(&["ACME".to_string(), "GAMMA".to_string()])
                .await
                .expect("degrees"),
            vec![1, 0]
        );

        graph
            .upsert_edges_batch(vec![
                (
                    "ACME".to_string(),
                    "GAMMA".to_string(),
                    json!({"weight": 1.0}),
                ),
                (
                    "GAMMA".to_string(),
                    "BETA".to_string(),
                    json!({"weight": 2.0}),
                ),
            ])
            .await
            .expect("batch edges");
        assert_eq!(
            graph
                .get_edges_batch(&[("ACME".into(), "GAMMA".into())])
                .await
                .expect("batch edges")[0]
                .as_ref()
                .expect("row")["weight"],
            1.0
        );

        let snapshot = graph.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.nodes.len(), 3);
        assert_eq!(snapshot.edges.len(), 3);
        assert_eq!(graph.node_degree("GAMMA").await.expect("degree"), 2);

        graph.index_done().await.expect("index done");
    }

    // Reopening sees committed state.
    let graph = TursoGraph::open(&path, "entity_graph")
        .await
        .expect("reopen");
    assert!(graph.has_node("ACME").await.expect("has"));
    assert!(graph.has_edge("ACME", "BETA").await.expect("has edge"));
    assert_eq!(
        graph.get_node("ACME").await.expect("get").expect("row")["source_id"],
        "chunk-1<SEP>chunk-2"
    );
    assert_eq!(graph.snapshot().await.expect("snapshot").edges.len(), 3);

    // A different scope in the same file is empty.
    let other = TursoGraph::open(&path, "other_graph")
        .await
        .expect("other scope");
    assert!(
        !other.has_node("ACME").await.expect("has"),
        "scope isolates graphs"
    );
}
