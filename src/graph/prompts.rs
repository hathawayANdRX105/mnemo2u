//! Prompt templates — byte-for-byte copies of the reference prompts
//! (`refs/nano-graphrag/nano_graphrag/prompt.py`), extracted by
//! `tools/golden/gen_prompts.py`.
//!
//! The text keeps the reference's `{placeholder}` syntax and doubled braces;
//! fill them with [`crate::core::text::fill_template`].

pub const ENTITY_EXTRACTION: &str = include_str!("prompts/entity_extraction.txt");
pub const ENTITI_CONTINUE_EXTRACTION: &str = include_str!("prompts/entiti_continue_extraction.txt");
pub const ENTITI_IF_LOOP_EXTRACTION: &str = include_str!("prompts/entiti_if_loop_extraction.txt");
pub const SUMMARIZE_ENTITY_DESCRIPTIONS: &str =
    include_str!("prompts/summarize_entity_descriptions.txt");
pub const COMMUNITY_REPORT: &str = include_str!("prompts/community_report.txt");
pub const LOCAL_RAG_RESPONSE: &str = include_str!("prompts/local_rag_response.txt");
pub const GLOBAL_MAP_RAG_POINTS: &str = include_str!("prompts/global_map_rag_points.txt");
pub const GLOBAL_REDUCE_RAG_RESPONSE: &str = include_str!("prompts/global_reduce_rag_response.txt");
pub const NAIVE_RAG_RESPONSE: &str = include_str!("prompts/naive_rag_response.txt");
pub const FAIL_RESPONSE: &str = include_str!("prompts/fail_response.txt");

/// `prompt.py:324` — the four default entity types.
pub const DEFAULT_ENTITY_TYPES: [&str; 4] = ["organization", "person", "geo", "event"];
/// `prompt.py:325`.
pub const DEFAULT_TUPLE_DELIMITER: &str = "<|>";
/// `prompt.py:326`.
pub const DEFAULT_RECORD_DELIMITER: &str = "##";
/// `prompt.py:327`.
pub const DEFAULT_COMPLETION_DELIMITER: &str = "<|COMPLETE|>";
/// `prompt.py:499` — separator hierarchy for `chunking_by_seperators`.
pub const DEFAULT_TEXT_SEPARATOR: [&str; 15] = [
    "\n\n", "\r\n\r\n", "\n", "\r\n", "。", "．", ".", "！", "!", "？", "?", " ", "\t", "　",
    "\u{200b}",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::{fill_template, GRAPH_FIELD_SEP};

    #[test]
    fn entity_extraction_prompt_has_reference_placeholders() {
        for placeholder in [
            "{tuple_delimiter}",
            "{record_delimiter}",
            "{completion_delimiter}",
            "{entity_types}",
            "{input_text}",
        ] {
            assert!(
                ENTITY_EXTRACTION.contains(placeholder),
                "missing {placeholder}"
            );
        }
    }

    #[test]
    fn fill_template_substitutes_and_unmatches_doubled_braces() {
        let filled = fill_template(
            ENTITY_EXTRACTION,
            &[
                ("tuple_delimiter", DEFAULT_TUPLE_DELIMITER),
                ("record_delimiter", DEFAULT_RECORD_DELIMITER),
                ("completion_delimiter", DEFAULT_COMPLETION_DELIMITER),
                ("entity_types", "organization,person,geo,event"),
                ("input_text", "ACME acquired Beta Labs."),
            ],
        )
        .expect("all placeholders supplied");
        assert!(!filled.contains("{input_text}"));
        assert!(!filled.contains("{tuple_delimiter}"));
        assert!(filled.contains("ACME acquired Beta Labs."));

        // Doubled braces are literals in Python format strings.
        let filled =
            fill_template(COMMUNITY_REPORT, &[("input_text", "X")]).expect("community report");
        assert!(!filled.contains("{{"), "doubled braces must collapse");

        // Unknown placeholders fail loudly instead of shipping a broken prompt.
        assert!(fill_template(ENTITY_EXTRACTION, &[]).is_err());
    }

    #[test]
    fn constants_match_reference() {
        assert_eq!(GRAPH_FIELD_SEP, "<SEP>");
        assert_eq!(DEFAULT_ENTITY_TYPES[0], "organization");
        assert_eq!(DEFAULT_TEXT_SEPARATOR[1], "\r\n\r\n");
    }
}
