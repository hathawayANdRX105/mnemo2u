//! Golden parity: incremental merge behaviour vs the LightRAG reference.
//!
//! Fixture produced by `scripts/gen_merge.py` (verbatim port of the reference
//! merge rules with a dict-graph mock). `source_id` is compared as a set: the
//! reference joins a Python `set`, whose iteration order is
//! implementation-defined. Descriptions compare exactly — the reference's
//! fragment order is deterministic (stored first, then new by
//! (timestamp, -length)).

use std::collections::BTreeSet;
use std::sync::Arc;

use mnemo2u::core::rag::{EntityRecord, RelationRecord};
use mnemo2u::core::text::Tokenizer;
use mnemo2u::core::traits::GraphStore;
use mnemo2u::index::merge::{merge_edge, merge_node, IndexMergeOptions};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockLlm;
use mnemo2u::store::memory::{MemoryGraph, MemoryKv};
use serde_json::Value;

fn as_set(value: &str) -> BTreeSet<String> {
    value.split("<SEP>").map(|part| part.to_string()).collect()
}

/// The fixture records `source_id` as a sorted list inside the expected
/// node/edge object (the reference joins a Python `set`).
fn expected_set(value: &Value) -> BTreeSet<String> {
    value["source_id"]
        .as_array()
        .expect("source id array")
        .iter()
        .map(|item| item.as_str().expect("string").to_string())
        .collect()
}

async fn seed_graph(golden: &Value) -> MemoryGraph {
    let graph = MemoryGraph::new();
    let seed = &golden["seed"];
    for (name, node) in seed["nodes"].as_object().expect("seed nodes") {
        graph
            .upsert_node(name, node.clone())
            .await
            .expect("seed node");
    }
    for (key, edge) in seed["edges"].as_object().expect("seed edges") {
        let mut parts = key.split('\u{0}');
        let src = parts.next().expect("src");
        let tgt = parts.next().expect("tgt");
        graph
            .upsert_edge(src, tgt, edge.clone())
            .await
            .expect("seed edge");
    }
    graph
}

fn records_from_json(value: &Value) -> Vec<EntityRecord> {
    value
        .as_array()
        .expect("records")
        .iter()
        .map(|record| EntityRecord {
            entity_name: record["entity_name"].as_str().expect("name").to_string(),
            entity_type: record["entity_type"].as_str().expect("type").to_string(),
            description: record["description"]
                .as_str()
                .expect("description")
                .to_string(),
            source_id: record["source_id"].as_str().expect("source").to_string(),
            file_path: record["file_path"].as_str().expect("file path").to_string(),
            timestamp: record["timestamp"].as_i64().expect("timestamp"),
        })
        .collect()
}

fn edge_records_from_json(value: &Value) -> Vec<RelationRecord> {
    value
        .as_array()
        .expect("records")
        .iter()
        .map(|record| RelationRecord {
            src_id: record["src_id"].as_str().expect("src").to_string(),
            tgt_id: record["tgt_id"].as_str().expect("tgt").to_string(),
            weight: record["weight"].as_f64().expect("weight"),
            description: record["description"]
                .as_str()
                .expect("description")
                .to_string(),
            source_id: record["source_id"].as_str().expect("source").to_string(),
            order: record["order"].as_i64().expect("order"),
            keywords: record["keywords"].as_str().expect("keywords").to_string(),
            file_path: record["file_path"].as_str().expect("file path").to_string(),
            timestamp: record["timestamp"].as_i64().expect("timestamp"),
        })
        .collect()
}

#[tokio::test]
async fn merge_matches_python_reference() {
    let raw = std::fs::read_to_string("tests/fixtures/merge_golden.json").expect("fixture");
    let cases: Value = serde_json::from_str(&raw).expect("valid json");
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");
    let options = IndexMergeOptions::default();

    for case in cases.as_array().expect("array") {
        let name = case["name"].as_str().expect("case name");
        let graph = seed_graph(case).await;
        let summary_response = case["summary_response"]
            .as_str()
            .expect("summary")
            .to_string();
        let inner = Arc::new(MockLlm::new("gpt-4o", vec![summary_response.clone()]));
        let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));

        match case["kind"].as_str().expect("kind") {
            "node" => {
                let entity = case["entity"].as_str().expect("entity");
                let records = records_from_json(&case["records"]);
                let merged = merge_node(entity, &records, &graph, &llm, &tokenizer, &options)
                    .await
                    .expect("merge node");

                let expected = &case["expected"];
                let want = &expected["node"];
                assert_eq!(
                    merged["entity_type"], want["entity_type"],
                    "entity_type in {name}"
                );
                assert_eq!(
                    merged["description"], want["description"],
                    "description in {name}"
                );
                assert_eq!(
                    merged["entity_name"], want["entity_name"],
                    "entity_name in {name}"
                );
                assert_eq!(
                    merged["file_path"], want["file_path"],
                    "file_path in {name}"
                );
                assert_eq!(
                    as_set(merged["source_id"].as_str().expect("source")),
                    expected_set(want),
                    "source_id set in {name}"
                );
                let stored = graph.get_node(entity).await.expect("read").expect("stored");
                assert_eq!(
                    stored["description"], merged["description"],
                    "stored description in {name}"
                );
                let expected_calls = expected["summary_calls"].as_array().expect("calls").len();
                assert_eq!(
                    inner.calls(),
                    expected_calls,
                    "summary call count in {name}"
                );
            }
            "edge" => {
                let src = case["src"].as_str().expect("src");
                let tgt = case["tgt"].as_str().expect("tgt");
                let records = edge_records_from_json(&case["records"]);
                let edge = merge_edge(src, tgt, &records, &graph, &llm, &tokenizer, &options)
                    .await
                    .expect("merge edge");

                let expected = &case["expected"];
                let want = &expected["edge"];
                assert_eq!(
                    edge["weight"].as_f64().expect("weight"),
                    want["weight"].as_f64().expect("expected weight"),
                    "weight in {name}"
                );
                assert_eq!(
                    edge["description"], want["description"],
                    "description in {name}"
                );
                assert_eq!(edge["keywords"], want["keywords"], "keywords in {name}");
                assert_eq!(edge["file_path"], want["file_path"], "file_path in {name}");
                assert_eq!(
                    as_set(edge["source_id"].as_str().expect("source")),
                    expected_set(want),
                    "edge source_id set in {name}"
                );
                let stored = graph.get_edge(src, tgt).await.expect("read").expect("edge");
                assert_eq!(
                    stored["keywords"], edge["keywords"],
                    "stored keywords in {name}"
                );
                let endpoint = graph
                    .get_node("BETA")
                    .await
                    .expect("read")
                    .expect("endpoint created");
                let want_endpoint = &expected["endpoint_node"];
                assert_eq!(
                    endpoint["entity_type"], want_endpoint["entity_type"],
                    "endpoint type in {name}"
                );
                assert_eq!(
                    endpoint["description"], want_endpoint["description"],
                    "endpoint description in {name}"
                );
                assert_eq!(
                    as_set(endpoint["source_id"].as_str().expect("source")),
                    as_set(want_endpoint["source_id"].as_str().expect("source")),
                    "endpoint source_id set in {name}"
                );
                let expected_calls = expected["summary_calls"].as_array().expect("calls").len();
                assert_eq!(
                    inner.calls(),
                    expected_calls,
                    "summary call count in {name}"
                );
            }
            other => panic!("unknown case kind {other}"),
        }
    }
}
