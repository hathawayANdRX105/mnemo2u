//! Golden parity: merge behaviour vs the reference implementation.
//!
//! Fixture produced by `tools/golden/gen_merge.py`. `source_id` is compared as
//! a set: the reference joins a Python `set` whose iteration order is
//! implementation-defined.

use std::collections::BTreeSet;
use std::sync::Arc;

use mnemo2u::core::rag::{EntityRecord, RelationRecord};
use mnemo2u::core::traits::GraphStore;
use mnemo2u::graph::merge::{merge_edges_then_upsert, merge_nodes_then_upsert, MergeOptions};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockLlm;
use mnemo2u::store::memory::{MemoryGraph, MemoryKv};
use mnemo2u::Tokenizer;
use serde_json::Value;

fn as_set(value: &str) -> BTreeSet<String> {
    value.split("<SEP>").map(|part| part.to_string()).collect()
}

fn expected_set(value: &Value, field: &str) -> BTreeSet<String> {
    value[field]
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
        })
        .collect()
}

#[tokio::test]
async fn merge_matches_python_reference() {
    let raw = std::fs::read_to_string("tests/fixtures/merge_golden.json").expect("fixture");
    let cases: Value = serde_json::from_str(&raw).expect("valid json");
    let tokenizer = Tokenizer::for_gpt_4o().expect("tokenizer");

    for case in cases.as_array().expect("array") {
        let name = case["name"].as_str().expect("case name");
        let graph = seed_graph(case).await;
        let summary_response = case["summary_response"]
            .as_str()
            .expect("summary")
            .to_string();
        let inner = Arc::new(MockLlm::new(
            "gpt-4o",
            vec![summary_response.clone(), summary_response.clone()],
        ));
        let llm = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));
        let options = MergeOptions::default();

        match case["kind"].as_str().expect("kind") {
            "node" => {
                let entity = case["entity"].as_str().expect("entity");
                let records = records_from_json(&case["records"]);
                let merged =
                    merge_nodes_then_upsert(entity, &records, &graph, &llm, &tokenizer, &options)
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
                    as_set(merged["source_id"].as_str().expect("source")),
                    expected_set(expected, "source_id"),
                    "source_id set in {name}"
                );
                // The graph must hold the same row.
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
                merge_edges_then_upsert(src, tgt, &records, &graph, &llm, &tokenizer, &options)
                    .await
                    .expect("merge edge");

                let expected = &case["expected"];
                let want = &expected["edge"];
                let edge = graph.get_edge(src, tgt).await.expect("read").expect("edge");
                assert_eq!(edge["weight"], want["weight"], "weight in {name}");
                assert_eq!(
                    edge["description"], want["description"],
                    "description in {name}"
                );
                assert_eq!(edge["order"], want["order"], "order in {name}");
                assert_eq!(
                    as_set(edge["source_id"].as_str().expect("source")),
                    expected_set(expected, "edge_source_id"),
                    "edge source_id set in {name}"
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
