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

/// CSV bytes are part of the ported contract: the reference emits numeric cells
/// **unquoted** (`enclose_string_with_quotes` returns `str(n)` for numbers) and
/// text cells quoted, joined with `",\t"`; the truncation key (`format_row`)
/// quotes *every* cell and doubles inner quotes. These two forms used to be
/// flattened to one, silently changing both the measured budget and the emitted
/// context, so pin them.
#[test]
fn csv_cells_quote_text_and_leave_numbers_bare() {
    use mnemo2u::core::text::{csv_measurement_row, list_of_list_to_csv, py_repr_float, CsvCell};

    let rows = vec![
        vec![
            CsvCell::text("id"),
            CsvCell::text("entity"),
            CsvCell::text("type"),
            CsvCell::text("description"),
            CsvCell::text("degree"),
        ],
        vec![
            CsvCell::int(0),
            CsvCell::text("\"ACME\""),
            CsvCell::text("\"ORGANIZATION\""),
            CsvCell::text("Acme makes things."),
            CsvCell::int(3),
        ],
    ];
    assert_eq!(
        list_of_list_to_csv(&rows),
        "\"id\",\t\"entity\",\t\"type\",\t\"description\",\t\"degree\"\n\
         0,\t\"ACME\",\t\"ORGANIZATION\",\t\"Acme makes things.\",\t3"
    );

    let data_row = &rows[1];
    assert_eq!(
        csv_measurement_row(data_row),
        "\"0\",\"\"\"ACME\"\"\",\"\"\"ORGANIZATION\"\"\",\"Acme makes things.\",\"3\""
    );

    // Python `str(float)`: 6.0 prints as "6.0", not "6".
    assert_eq!(py_repr_float(6.0), "6.0");
    assert_eq!(py_repr_float(0.5), "0.5");
}
