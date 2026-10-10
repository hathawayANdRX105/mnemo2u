//! Golden parity: extraction record parser vs the reference implementation.
//!
//! Fixture produced by `tools/golden/gen_extraction.py` (verbatim copy of the
//! reference `_handle_single_*` helpers and the record loop).

use mnemo2u::graph::extract::parse_extraction_result;
use serde_json::Value;

#[test]
fn extraction_parser_matches_python_reference() {
    let raw = std::fs::read_to_string("tests/fixtures/extraction_golden.json").expect("fixture");
    let cases: Value = serde_json::from_str(&raw).expect("valid json");
    let cases = cases.as_array().expect("array of cases");
    assert!(!cases.is_empty());

    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let chunk_key = case["chunk_key"].as_str().expect("chunk key");
        let raw_text = case["raw"].as_str().expect("raw output");
        let file_path = case["file_path"].as_str().expect("file path");
        let parsed = parse_extraction_result(raw_text, chunk_key, file_path);

        let expected_nodes = case["expected"]["nodes"].as_array().expect("nodes");
        assert_eq!(
            parsed.nodes.len(),
            expected_nodes.len(),
            "node group count in {name}"
        );
        for (got, want) in parsed.nodes.iter().zip(expected_nodes) {
            assert_eq!(
                got.0,
                want[0].as_str().expect("node name"),
                "node name in {name}"
            );
            let want_records = want[1].as_array().expect("records");
            assert_eq!(
                got.1.len(),
                want_records.len(),
                "record count for {} in {name}",
                got.0
            );
            for (record, want_record) in got.1.iter().zip(want_records) {
                assert_eq!(
                    record.entity_name,
                    want_record["entity_name"].as_str().expect("name")
                );
                assert_eq!(
                    record.entity_type,
                    want_record["entity_type"].as_str().expect("type")
                );
                assert_eq!(
                    record.description,
                    want_record["description"].as_str().expect("desc")
                );
                assert_eq!(
                    record.source_id,
                    want_record["source_id"].as_str().expect("source")
                );
                assert_eq!(
                    record.file_path,
                    want_record["file_path"].as_str().expect("file path")
                );
            }
        }

        let expected_edges = case["expected"]["edges"].as_array().expect("edges");
        assert_eq!(
            parsed.edges.len(),
            expected_edges.len(),
            "edge group count in {name}"
        );
        for (got, want) in parsed.edges.iter().zip(expected_edges) {
            let want_key = want[0].as_array().expect("edge key");
            assert_eq!(
                got.0 .0,
                want_key[0].as_str().expect("src"),
                "edge src in {name}"
            );
            assert_eq!(
                got.0 .1,
                want_key[1].as_str().expect("tgt"),
                "edge tgt in {name}"
            );
            let want_records = want[1].as_array().expect("edge records");
            assert_eq!(got.1.len(), want_records.len(), "edge records in {name}");
            for (record, want_record) in got.1.iter().zip(want_records) {
                assert_eq!(record.src_id, want_record["src_id"].as_str().expect("src"));
                assert_eq!(record.tgt_id, want_record["tgt_id"].as_str().expect("tgt"));
                assert_eq!(
                    record.weight,
                    want_record["weight"].as_f64().expect("weight")
                );
                assert_eq!(
                    record.keywords,
                    want_record["keywords"].as_str().expect("keywords")
                );
                assert_eq!(
                    record.description,
                    want_record["description"].as_str().expect("desc")
                );
                assert_eq!(
                    record.source_id,
                    want_record["source_id"].as_str().expect("source")
                );
                assert_eq!(
                    record.file_path,
                    want_record["file_path"].as_str().expect("file path")
                );
            }
        }
    }
}
