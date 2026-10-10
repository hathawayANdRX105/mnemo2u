//! Keyword-response parsing — the strict JSON contract
//! (`src/query/keywords.rs`, reference `extract_keywords_only`).

use mnemo2u::query::keywords::parse_keywords_response;

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
