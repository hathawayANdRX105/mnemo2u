//! Golden parity: cache-key hashing vs Python `compute_args_hash`.
//!
//! Fixture produced by `tools/golden/gen_hash.py`.

use mnemo2u::core::rag::ChatMessage;
use mnemo2u::core::text::compute_args_hash;
use serde_json::Value;

#[test]
fn args_hash_matches_python_md5_of_str_tuple() {
    let raw = std::fs::read_to_string("tests/fixtures/hash_golden.json").expect("fixture exists");
    let cases: Value = serde_json::from_str(&raw).expect("valid json");
    let cases = cases.as_array().expect("array of cases");
    assert!(!cases.is_empty());

    for case in cases {
        let model = case["model"].as_str().expect("model");
        let messages: Vec<ChatMessage> = case["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| ChatMessage {
                role: m["role"].as_str().expect("role").to_string(),
                content: m["content"].as_str().expect("content").to_string(),
            })
            .collect();
        let expected = case["expected_md5"].as_str().expect("expected");
        assert_eq!(compute_args_hash(model, &messages), expected, "case {case}");
    }
}
