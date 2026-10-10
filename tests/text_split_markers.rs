//! Byte-parity: `split_string_by_multi_markers` vs LightRAG `utils.py:3997`.
//!
//! The reference is `re.split` on `re.escape`d markers joined by `|` — the
//! markers are literals and, at one position, the alternative that appears
//! earlier in the list wins; parts are stripped and empty ones dropped. This
//! engine's `regex::escape` emits `\<`/`\>` (word-boundary assertions, not
//! literals), so the Rust side is a plain leftmost scan instead. These tests
//! pin that the scan agrees with the reference on the load-bearing cases:
//! a marker that literally contains `<`/`>`, marker ordering, empty-segment
//! dropping and multi-byte input.

use mnemo2u::core::text::split_string_by_multi_markers;

/// The record delimiter `<|#|>` must split as a literal (the exact case the
/// regex engine broke on).
#[test]
fn tuple_delimiter_splits_as_a_literal() {
    assert_eq!(
        split_string_by_multi_markers("entity<|#|>ACME<|#|>organization", &["<|#|>"]),
        vec!["entity", "ACME", "organization"]
    );
}

/// No markers: the whole content comes back as a single element.
#[test]
fn empty_marker_list_returns_whole_content() {
    assert_eq!(
        split_string_by_multi_markers("hello world", &[]),
        vec!["hello world"]
    );
}

/// Parts are trimmed and empty segments dropped (leading/trailing/middle).
#[test]
fn parts_are_trimmed_and_empties_dropped() {
    assert_eq!(
        split_string_by_multi_markers("a,,b", &[","]),
        vec!["a", "b"]
    );
    assert_eq!(
        split_string_by_multi_markers(", a , b ,", &[","]),
        vec!["a", "b"]
    );
}

/// At one position the marker earlier in the list wins (the reference's
/// alternation order). The two orderings over the same content disagree,
/// which pins the leftmost-in-list scan.
#[test]
fn earlier_marker_in_the_list_wins_at_a_position() {
    assert_eq!(
        split_string_by_multi_markers("xyabzw", &["abzw", "ab"]),
        vec!["xy"]
    );
    assert_eq!(
        split_string_by_multi_markers("xyabzw", &["ab", "abzw"]),
        vec!["xy", "zw"]
    );
}

/// The scan advances by whole characters: a marker placed next to multi-byte
/// text must not corrupt the CJK segments.
#[test]
fn multibyte_segments_survive_the_split() {
    assert_eq!(
        split_string_by_multi_markers("实体<SEP>名称<SEP>其他", &["<SEP>"]),
        vec!["实体", "名称", "其他"]
    );
}

/// The realistic extraction shape: newline record split, then the `<|#|>`
/// field split inside each record.
#[test]
fn extraction_records_then_fields() {
    let records = split_string_by_multi_markers(
        "entity<|#|>ACME<|#|>organization<|#|>desc\nrelation<|#|>ACME<|#|>BETA",
        &["\n"],
    );
    assert_eq!(
        records,
        vec![
            "entity<|#|>ACME<|#|>organization<|#|>desc",
            "relation<|#|>ACME<|#|>BETA",
        ]
    );
    assert_eq!(
        split_string_by_multi_markers(&records[0], &["<|#|>"]),
        vec!["entity", "ACME", "organization", "desc"]
    );
}
