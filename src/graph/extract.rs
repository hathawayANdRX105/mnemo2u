//! Entity/relation extraction — port of `_op.py::extract_entities` (282-414),
//! including the gleaning loop and the record parser (`_handle_single_*`, :138-179).

use tokio::task::JoinSet;

use crate::core::concurrency::Limiter;
use crate::core::rag::{ChatMessage, Chunk, EntityRecord, RelationRecord};
use crate::core::text::{clean_str, fill_template, is_float_regex, split_string_by_multi_markers};
use crate::graph::prompts::{
    DEFAULT_COMPLETION_DELIMITER, DEFAULT_ENTITY_TYPES, DEFAULT_RECORD_DELIMITER,
    DEFAULT_TUPLE_DELIMITER, ENTITI_CONTINUE_EXTRACTION, ENTITI_IF_LOOP_EXTRACTION,
    ENTITY_EXTRACTION,
};
use crate::llm::cache::CachedLlm;
use crate::llm::{LlmError, LlmResult, ModelOptions};

/// `entity_extract_max_gleaning` default (`graphrag.py:83`).
pub const DEFAULT_MAX_GLEANING: usize = 1;
/// `best_model_max_async` default (`graphrag.py:119`).
pub const DEFAULT_BEST_MODEL_MAX_ASYNC: usize = 16;

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub max_gleaning: usize,
    pub best_model_max_async: usize,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            max_gleaning: DEFAULT_MAX_GLEANING,
            best_model_max_async: DEFAULT_BEST_MODEL_MAX_ASYNC,
        }
    }
}

/// Aggregated extraction output, in first-encounter order (reference
/// `defaultdict` semantics). Edge keys are sorted pairs (undirected graph).
#[derive(Debug, Default, PartialEq)]
pub struct ExtractedRecords {
    pub nodes: Vec<(String, Vec<EntityRecord>)>,
    pub edges: Vec<((String, String), Vec<RelationRecord>)>,
}

impl ExtractedRecords {
    fn push_node(&mut self, entity_name: String, record: EntityRecord) {
        match self.nodes.iter_mut().find(|(name, _)| *name == entity_name) {
            Some((_, records)) => records.push(record),
            None => self.nodes.push((entity_name, vec![record])),
        }
    }

    fn push_edge(&mut self, key: (String, String), record: RelationRecord) {
        match self.edges.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, records)) => records.push(record),
            None => self.edges.push((key, vec![record])),
        }
    }
}

/// Parse one aggregation of raw LLM records for a chunk
/// (`_process_single_content` tail + `_handle_single_*`).
pub fn parse_extraction_result(final_result: &str, chunk_key: &str) -> ExtractedRecords {
    let records = split_string_by_multi_markers(
        final_result,
        &[DEFAULT_RECORD_DELIMITER, DEFAULT_COMPLETION_DELIMITER],
    );
    let mut extracted = ExtractedRecords::default();
    for record in records {
        let Some(group) = search_parenthesised(&record) else {
            continue;
        };
        let attributes = split_string_by_multi_markers(&group, &[DEFAULT_TUPLE_DELIMITER]);
        if let Some(entity) = handle_single_entity_extraction(&attributes, chunk_key) {
            extracted.push_node(entity.entity_name.clone(), entity);
            continue;
        }
        if let Some(relation) = handle_single_relationship_extraction(&attributes, chunk_key) {
            let key = if relation.src_id <= relation.tgt_id {
                (relation.src_id.clone(), relation.tgt_id.clone())
            } else {
                (relation.tgt_id.clone(), relation.src_id.clone())
            };
            extracted.push_edge(key, relation);
        }
    }
    extracted
}

/// `re.search(r"\((.*)\)", record)` — leftmost parenthesised group; `.` does
/// not cross newlines, so the match must live on a single line.
fn search_parenthesised(record: &str) -> Option<String> {
    for line in record.split('\n') {
        if let Some(open) = line.find('(') {
            let tail = &line[open + 1..];
            if let Some(close) = tail.rfind(')') {
                return Some(tail[..close].to_string());
            }
        }
    }
    None
}

/// `_handle_single_entity_extraction` (`_op.py:138-156`).
pub fn handle_single_entity_extraction(
    attributes: &[String],
    chunk_key: &str,
) -> Option<EntityRecord> {
    if attributes.len() < 4 || attributes[0] != "\"entity\"" {
        return None;
    }
    let entity_name = clean_str(&attributes[1].to_uppercase());
    if entity_name.trim().is_empty() {
        return None;
    }
    Some(EntityRecord {
        entity_name,
        entity_type: clean_str(&attributes[2].to_uppercase()),
        description: clean_str(&attributes[3]),
        source_id: chunk_key.to_string(),
    })
}

/// `_handle_single_relationship_extraction` (`_op.py:159-179`).
pub fn handle_single_relationship_extraction(
    attributes: &[String],
    chunk_key: &str,
) -> Option<RelationRecord> {
    if attributes.len() < 5 || attributes[0] != "\"relationship\"" {
        return None;
    }
    let strength = attributes.last().expect("length checked");
    let weight = if is_float_regex(strength) {
        strength.parse::<f64>().unwrap_or(1.0)
    } else {
        1.0
    };
    Some(RelationRecord {
        src_id: clean_str(&attributes[1].to_uppercase()),
        tgt_id: clean_str(&attributes[2].to_uppercase()),
        weight,
        description: clean_str(&attributes[3]),
        source_id: chunk_key.to_string(),
        order: 1,
    })
}

/// Run extraction over a chunk set with the reference's gleaning loop,
/// concurrently, capped by [`ExtractOptions::best_model_max_async`].
pub async fn extract_entities(
    chunks: &[(String, Chunk)],
    llm: &CachedLlm,
    options: &ExtractOptions,
) -> LlmResult<ExtractedRecords> {
    let limiter = Limiter::new(options.best_model_max_async);
    let entity_types = DEFAULT_ENTITY_TYPES.join(",");

    let mut set: JoinSet<(usize, LlmResult<ExtractedRecords>)> = JoinSet::new();
    for (index, (chunk_key, chunk)) in chunks.iter().enumerate() {
        let chunk_key = chunk_key.clone();
        let content = chunk.content.clone();
        let llm = llm.clone();
        let limiter = limiter.clone();
        let options = options.clone();
        let entity_types = entity_types.clone();
        set.spawn(async move {
            let result = process_single_content(
                &chunk_key,
                &content,
                &llm,
                &limiter,
                &options,
                &entity_types,
            )
            .await;
            (index, result)
        });
    }

    let mut ordered: Vec<Option<LlmResult<ExtractedRecords>>> =
        (0..chunks.len()).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        let (index, result) = joined.map_err(|e| LlmError::Transport(format!("join: {e}")))?;
        ordered[index] = Some(result);
    }

    let mut aggregated = ExtractedRecords::default();
    for slot in ordered.into_iter() {
        let records = slot.expect("every chunk task reports back")?;
        for (name, mut values) in records.nodes {
            match aggregated
                .nodes
                .iter_mut()
                .find(|(existing, _)| *existing == name)
            {
                Some((_, existing)) => existing.append(&mut values),
                None => aggregated.nodes.push((name, values)),
            }
        }
        for (key, mut values) in records.edges {
            match aggregated
                .edges
                .iter_mut()
                .find(|(existing, _)| *existing == key)
            {
                Some((_, existing)) => existing.append(&mut values),
                None => aggregated.edges.push((key, values)),
            }
        }
    }
    Ok(aggregated)
}

/// One chunk: initial extraction, gleaning loop, then parsing
/// (`_process_single_content`, `_op.py:318-394`).
async fn process_single_content(
    chunk_key: &str,
    content: &str,
    llm: &CachedLlm,
    limiter: &Limiter,
    options: &ExtractOptions,
    entity_types: &str,
) -> LlmResult<ExtractedRecords> {
    let hint_prompt = fill_template(
        ENTITY_EXTRACTION,
        &[
            ("tuple_delimiter", DEFAULT_TUPLE_DELIMITER),
            ("record_delimiter", DEFAULT_RECORD_DELIMITER),
            ("completion_delimiter", DEFAULT_COMPLETION_DELIMITER),
            ("entity_types", entity_types),
            ("input_text", content),
        ],
    )
    .map_err(|e| LlmError::Decode(format!("prompt template: {e}")))?;

    let (mut final_result, _) = limiter
        .run(llm.complete_cached(&hint_prompt, None, &[], &ModelOptions::default()))
        .await?;
    let mut history = vec![
        ChatMessage::user(hint_prompt),
        ChatMessage::assistant(final_result.clone()),
    ];

    for glean_index in 0..options.max_gleaning {
        let (glean_result, _) = limiter
            .run(llm.complete_cached(
                ENTITI_CONTINUE_EXTRACTION,
                None,
                &history,
                &ModelOptions::default(),
            ))
            .await?;
        history.push(ChatMessage::user(ENTITI_CONTINUE_EXTRACTION.to_string()));
        history.push(ChatMessage::assistant(glean_result.clone()));
        final_result.push_str(&glean_result);
        if glean_index + 1 == options.max_gleaning {
            break;
        }
        let (loop_answer, _) = limiter
            .run(llm.complete_cached(
                ENTITI_IF_LOOP_EXTRACTION,
                None,
                &history,
                &ModelOptions::default(),
            ))
            .await?;
        let normalized = loop_answer
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_lowercase();
        if normalized != "yes" {
            break;
        }
    }

    Ok(parse_extraction_result(&final_result, chunk_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_quoted_records() {
        let raw = "(\"entity\"<|>\"ACME\"<|>\"ORGANIZATION\"<|>\"Acme makes things.\")##\
                   (\"relationship\"<|>\"ACME\"<|>\"BETA\"<|>\"owns\"<|>7.5)<|COMPLETE|>";
        let parsed = parse_extraction_result(raw, "chunk-1");
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].0, "\"ACME\"");
        assert_eq!(parsed.nodes[0].1[0].entity_type, "\"ORGANIZATION\"");
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.edges[0].1[0].weight, 7.5);
    }

    #[test]
    fn parser_skips_malformed_records() {
        // Records are split on the record delimiter first (reference
        // `split_string_by_multi_markers`), so each malformed case needs its
        // own record.
        let raw = "no parens here##\
(\"entity\"<|>\"A\"<|>\"B\")##\
(\"relationship\"<|>\"A\"<|>\"B\"<|>\"d\"<|>not-a-number)";
        let parsed = parse_extraction_result(raw, "c");
        assert!(
            parsed.nodes.is_empty(),
            "three-attribute entity record must be rejected"
        );
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(
            parsed.edges[0].1[0].weight, 1.0,
            "non-numeric strength falls back to 1.0"
        );
    }

    #[test]
    fn parser_requires_the_entity_marker() {
        let raw = "(\"thing\"<|>\"A\"<|>\"B\"<|>\"d\")";
        assert!(parse_extraction_result(raw, "c").nodes.is_empty());
    }
}
