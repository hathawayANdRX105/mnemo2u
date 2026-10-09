//! Community detection + report generation over the in-memory graph.

use std::sync::Arc;

use mnemo2u::core::traits::GraphStore;
use mnemo2u::graph::community::{detect_communities, CommunityOptions};
use mnemo2u::graph::reports::{
    community_report_json_to_str, generate_community_report, ReportOptions,
};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::RoutedLlm;
use mnemo2u::store::memory::{MemoryGraph, MemoryKv};
use mnemo2u::Tokenizer;
use serde_json::json;

const REPORT: &str = r#"{"title": "Community", "summary": "Two clusters linked.", "findings": [{"summary": "Bridge", "explanation": "A1 and B1 talk."}], "rating": 6}"#;

async fn build_graph() -> MemoryGraph {
    let graph = MemoryGraph::new();
    for (index, (name, source)) in [
        ("A1", "chunk-a1"),
        ("A2", "chunk-a2"),
        ("A3", "chunk-a3"),
        ("B1", "chunk-b1"),
        ("B2", "chunk-b2"),
        ("B3", "chunk-b3"),
        ("LONE", "chunk-lone"),
    ]
    .into_iter()
    .enumerate()
    {
        graph
            .upsert_node(
                name,
                json!({
                    "entity_type": "ORG",
                    "description": format!("entity {name}"),
                    "source_id": source,
                    "rank": index,
                }),
            )
            .await
            .expect("node");
    }
    for (src, tgt) in [
        ("A1", "A2"),
        ("A2", "A3"),
        ("A1", "A3"),
        ("B1", "B2"),
        ("B2", "B3"),
        ("B1", "B3"),
        ("A1", "B1"),
    ] {
        graph
            .upsert_edge(
                src,
                tgt,
                json!({"weight": 3.0, "description": format!("{src}-{tgt}"), "order": 1}),
            )
            .await
            .expect("edge");
    }
    graph
}

#[tokio::test]
async fn detection_writes_clusters_and_builds_schema() {
    let graph = build_graph().await;
    let result = detect_communities(&graph, &CommunityOptions::default())
        .await
        .expect("detect");

    assert_eq!(
        result.memberships.len(),
        6,
        "the isolated node is not clustered"
    );
    assert!(!result.schema.is_empty());

    let clusters = graph
        .get_node("A1")
        .await
        .expect("read")
        .expect("node")
        .get("clusters")
        .cloned()
        .expect("clusters attribute written back");
    let parsed: Vec<serde_json::Value> = serde_json::from_value(clusters).expect("json clusters");
    assert!(!parsed.is_empty());
    assert!(parsed[0]["level"].is_i64());

    let lone = graph.get_node("LONE").await.expect("read").expect("node");
    assert!(
        lone.get("clusters").is_none(),
        "nodes outside the component stay unclustered"
    );

    for (key, community) in &result.schema {
        assert_eq!(community.title, format!("Cluster {key}"));
        assert!(community.occurrence > 0.0 && community.occurrence <= 1.0);
        assert!(!community.nodes.is_empty());
    }
}

#[tokio::test]
async fn reports_are_generated_per_community_and_formatted() {
    let graph = build_graph().await;
    let detection = detect_communities(&graph, &CommunityOptions::default())
        .await
        .expect("detect");

    let llm = Arc::new(RoutedLlm::new(
        "mock-best",
        vec![(
            "general information discovery".to_string(),
            REPORT.to_string(),
        )],
    ));
    let cached = CachedLlm::new(llm.clone(), Arc::new(MemoryKv::new()));
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");

    let reports = generate_community_report(
        &graph,
        &detection.schema,
        &cached,
        &tokenizer,
        &ReportOptions::default(),
        4,
    )
    .await
    .expect("reports");

    assert_eq!(reports.len(), detection.schema.len());
    assert_eq!(
        llm.calls(),
        detection.schema.len(),
        "one call per community"
    );
    for (_, community) in &reports {
        let text = community.report_string.clone().expect("report string");
        assert!(
            text.starts_with("# Community\n\nTwo clusters linked."),
            "unexpected text: {text}"
        );
        assert!(text.contains("## Bridge\n\nA1 and B1 talk."));
        let parsed = community.report_json.clone().expect("report json");
        assert_eq!(parsed["rating"], 6);
    }

    // The JSON→markdown conversion matches the reference shape.
    let formatted = community_report_json_to_str(&json!({
        "title": "T",
        "summary": "S",
        "findings": ["plain string finding"],
    }));
    assert_eq!(formatted, "# T\n\nS\n\n## plain string finding\n\n");
}
