//! Prompt templates — byte-for-byte copies of the LightRAG prompts
//! (`refs/LightRAG/lightrag/prompt.py`), extracted by `scripts/gen_prompts.py`.
//!
//! The text keeps the reference's `{placeholder}` syntax and doubled braces;
//! fill them with [`crate::core::text::fill_template`].
//!
//! R1 ported the nano-graphrag prompts (single-user-prompt extraction,
//! community-report global search). R2 replaced that surface with the LightRAG
//! one: system+user extraction prompts, JSON keyword extraction, and the
//! unified `rag_response`/`kg_query_context` rendering.

pub const ENTITY_EXTRACTION_SYSTEM: &str = include_str!("prompts/entity_extraction_system.txt");
pub const ENTITY_EXTRACTION_USER: &str = include_str!("prompts/entity_extraction_user.txt");
pub const ENTITY_CONTINUE_EXTRACTION: &str = include_str!("prompts/entity_continue_extraction.txt");
pub const SUMMARIZE_ENTITY_DESCRIPTIONS: &str =
    include_str!("prompts/summarize_entity_descriptions.txt");
pub const KEYWORDS_EXTRACTION: &str = include_str!("prompts/keywords_extraction.txt");
pub const RAG_RESPONSE: &str = include_str!("prompts/rag_response.txt");
pub const FAIL_RESPONSE: &str = include_str!("prompts/fail_response.txt");
pub const KG_QUERY_CONTEXT: &str = include_str!("prompts/kg_query_context.txt");
pub const NAIVE_QUERY_CONTEXT: &str = include_str!("prompts/naive_query_context.txt");
pub const NAIVE_RAG_RESPONSE: &str = include_str!("prompts/naive_rag_response.txt");
pub const COMMUNITY_REPORT: &str = include_str!("prompts/community_report.txt");

/// `prompt.py:14` — field separator inside one record.
pub const DEFAULT_TUPLE_DELIMITER: &str = "<|#|>";
/// `prompt.py:15` — end-of-extraction marker.
pub const DEFAULT_COMPLETION_DELIMITER: &str = "<|COMPLETE|>";
/// `GRAGH_FIELD_SEP` (`constants.py:49`): the `source_id` list separator.
pub const GRAPH_FIELD_SEP: &str = "<SEP>";

/// `DEFAULT_MAX_EXTRACTION_RECORDS` (`constants.py:26`).
pub const DEFAULT_MAX_EXTRACTION_RECORDS: usize = 100;
/// `DEFAULT_MAX_EXTRACTION_ENTITIES` (`constants.py:27`).
pub const DEFAULT_MAX_EXTRACTION_ENTITIES: usize = 40;
/// `DEFAULT_MAX_EXTRACT_INPUT_TOKENS` (`constants.py:38`).
pub const DEFAULT_MAX_EXTRACT_INPUT_TOKENS: usize = 20_480;
/// `DEFAULT_FORCE_LLM_SUMMARY_ON_MERGE` (`constants.py:30`).
pub const DEFAULT_FORCE_LLM_SUMMARY_ON_MERGE: usize = 8;
/// `DEFAULT_SUMMARY_MAX_TOKENS` (`constants.py:32`).
pub const DEFAULT_SUMMARY_MAX_TOKENS: usize = 1_200;
/// `DEFAULT_SUMMARY_CONTEXT_SIZE` (`constants.py:36`).
pub const DEFAULT_SUMMARY_CONTEXT_SIZE: usize = 12_000;
/// `DEFAULT_SUMMARY_LENGTH_RECOMMENDED` (`constants.py:34`).
pub const DEFAULT_SUMMARY_LENGTH_RECOMMENDED: usize = 600;
/// `DEFAULT_MAX_GLEANING` — the reference runs exactly one extra round when the
/// value is positive (`operate.py:4252-4253`).
pub const DEFAULT_MAX_GLEANING: usize = 1;

/// The example row block the reference feeds the system prompt
/// (`prompt.py::entity_extraction_examples`, formatted once per run at
/// `operate.py:4044`).
pub const EXTRACTION_EXAMPLES: &str = "entity{tuple_delimiter}<entity_name>{tuple_delimiter}<entity_type>{tuple_delimiter}<entity_description>\nrelation{tuple_delimiter}<source_entity>{tuple_delimiter}<target_entity>{tuple_delimiter}<relationship_keywords>{tuple_delimiter}<relationship_description>\n{completion_delimiter}\n";

/// Query-keyword output example (`prompt.py::keywords_extraction_examples`).
pub const KEYWORDS_EXTRACTION_EXAMPLES: &str = r#"{
  "high_level_keywords": ["<high_level_keyword>", ...],
  "low_level_keywords": ["<low_level_keyword>", ...]
}"#;

/// Entity type guidance block (`prompt.py::default_entity_types_guidance`,
/// generated into `prompts/constants.json`).
pub const ENTITY_TYPES_GUIDANCE: &str = include_str!("prompts/entity_types_guidance.txt");
