//! Keyword-response parsing — the strict JSON contract
//! (`src/query/keywords.rs`, reference `extract_keywords_only`).

use std::sync::Arc;

use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockLlm;
use mnemo2u::query::keywords::{extract_keywords_only, parse_keywords_response};
use mnemo2u::store::memory::MemoryKv;

#[test]
fn exact_shape_parses() {
    let text = r#"{"high_level_keywords": ["ownership"], "low_level_keywords": ["ACME", "BETA"]}"#;
    let parsed = parse_keywords_response(text).expect("parses");
    assert_eq!(parsed.high, vec!["ownership".to_string()]);
    assert_eq!(parsed.low, vec!["ACME".to_string(), "BETA".to_string()]);
}

#[test]
fn empty_lists_parse() {
    let parsed =
        parse_keywords_response(r#"{"high_level_keywords": [], "low_level_keywords": []}"#)
            .expect("parses");
    assert!(parsed.high.is_empty());
    assert!(parsed.low.is_empty());
}

#[test]
fn fenced_payload_tolerated() {
    let text = "```json\n{\"high_level_keywords\": [\"a\"], \"low_level_keywords\": [\"b\"]}\n```";
    let parsed = parse_keywords_response(text).expect("parses");
    assert_eq!(parsed.high, vec!["a".to_string()]);
    assert_eq!(parsed.low, vec!["b".to_string()]);
}

#[test]
fn prose_before_the_object_is_ignored() {
    let text =
        "Here you go:\n{\"high_level_keywords\": [\"a\"], \"low_level_keywords\": [\"b\"]}\nDone.";
    let parsed = parse_keywords_response(text).expect("parses");
    assert_eq!(parsed.high, vec!["a".to_string()]);
}

#[test]
fn extra_keys_are_rejected() {
    let text = r#"{"high_level_keywords": ["a"], "low_level_keywords": ["b"], "meta": "x"}"#;
    assert!(parse_keywords_response(text).is_none());
}

#[test]
fn missing_key_is_rejected() {
    let text = r#"{"high_level_keywords": ["a"]}"#;
    assert!(parse_keywords_response(text).is_none());
}

#[test]
fn non_string_entries_are_rejected() {
    let text = r#"{"high_level_keywords": [1, 2], "low_level_keywords": ["b"]}"#;
    assert!(parse_keywords_response(text).is_none());
}

#[test]
fn unbalanced_braces_are_rejected() {
    let text = r#"{"high_level_keywords": ["a"], "low_level_keywords": ["b"]"#;
    assert!(parse_keywords_response(text).is_none());
}

#[test]
fn no_object_at_all_is_rejected() {
    assert!(parse_keywords_response("just prose").is_none());
    assert!(parse_keywords_response("").is_none());
}

#[test]
fn first_of_two_objects_is_used() {
    let text = r#"{"high_level_keywords": ["first"], "low_level_keywords": []} {"x": 1}"#;
    let parsed = parse_keywords_response(text).expect("parses");
    assert_eq!(parsed.high, vec!["first".to_string()]);
}

const GOOD: &str = r#"{"high_level_keywords": ["themes"], "low_level_keywords": ["ACME"]}"#;

/// A response that fails to parse once must be retried against the model, and
/// the retry's payload must win without falling back to empty lists
/// (`extract_keywords_only`, retry cap per the reference's `max_retries=3`).
#[tokio::test]
async fn unparseable_response_is_retried_until_it_parses() {
    let inner = Arc::new(MockLlm::new(
        "mock",
        vec!["just prose, no json anywhere".to_string(), GOOD.to_string()],
    ));
    let client = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));

    let keywords = extract_keywords_only(&client, "who owns ACME?")
        .await
        .expect("keywords");
    assert_eq!(keywords.high, vec!["themes".to_string()]);
    assert_eq!(keywords.low, vec!["ACME".to_string()]);
    assert_eq!(inner.calls(), 2, "first attempt plus one successful retry");

    // The retry replaced the cached bad payload: the repeat query is a cache
    // hit on the good payload and burns no model call.
    let again = extract_keywords_only(&client, "who owns ACME?")
        .await
        .expect("keywords");
    assert_eq!(again, keywords);
    assert_eq!(
        inner.calls(),
        2,
        "the replaced cache row must serve the repeat"
    );
}

/// Every attempt unparseable: the retry cap is burned and the query falls
/// back to empty keyword lists instead of panicking or erroring.
#[tokio::test]
async fn repeated_parse_failures_fall_back_to_empty_keyword_lists() {
    // One bad response per call: first attempt + the 3-retry cap = 4. A
    // 5th attempt would exhaust this script and fail the test.
    let bad: Vec<String> = (0..4).map(|i| format!("still no json {i}")).collect();
    let inner = Arc::new(MockLlm::new("mock", bad));
    let client = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));

    let keywords = extract_keywords_only(&client, "query")
        .await
        .expect("no panic, no error");
    assert!(keywords.high.is_empty());
    assert!(keywords.low.is_empty());
    assert_eq!(inner.calls(), 4, "one attempt plus the retry cap, no more");
}

/// A parseable first response must never pay for a retry.
#[tokio::test]
async fn parseable_first_response_never_retries() {
    let inner = Arc::new(MockLlm::new("mock", vec![GOOD.to_string()]));
    let client = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));

    let keywords = extract_keywords_only(&client, "query")
        .await
        .expect("keywords");
    assert_eq!(keywords.high, vec!["themes".to_string()]);
    assert_eq!(inner.calls(), 1, "a parseable payload needs no retry");
}
