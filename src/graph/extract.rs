//! Entity/relation extraction — port of `lightrag/operate.py::extract_entities`
//! (3942-4340): a chunk-invariant system prompt, a per-chunk user prompt, one
//! optional gleaning round, and the record parsers
//! (`_handle_single_entity_extraction` :710, `_handle_single_relationship_extraction`
//! :774).
//!
//! The tuple layout is LightRAG's: entities carry 4 fields, relations carry 5
//! (`relation, src, tgt, keywords, description`) with a fixed weight of 1.0 —
//! the nano-graphrag strength field is gone.

use tokio::task::JoinSet;

use crate::core::concurrency::Limiter;
use crate::core::rag::{ChatMessage, Chunk, EntityRecord, RelationRecord};
use crate::core::text::Tokenizer;
use crate::core::text::{
    fill_template, normalize_entity_name, sanitize_and_normalize_extracted_text,
    split_string_by_multi_markers,
};
use crate::graph::prompts::{
    DEFAULT_COMPLETION_DELIMITER, DEFAULT_MAX_EXTRACTION_ENTITIES, DEFAULT_MAX_EXTRACTION_RECORDS,
    DEFAULT_MAX_EXTRACT_INPUT_TOKENS, DEFAULT_MAX_GLEANING, DEFAULT_TUPLE_DELIMITER,
    ENTITY_CONTINUE_EXTRACTION, ENTITY_EXTRACTION_SYSTEM, ENTITY_EXTRACTION_USER,
    ENTITY_TYPES_GUIDANCE, EXTRACTION_EXAMPLES,
};
use crate::llm::cache::CachedLlm;
use crate::llm::{LlmError, LlmResult, ModelOptions};

/// `best_model_max_async` default (`lightrag.py:119`); the reference caps the
/// extract pool at this width.
pub const DEFAULT_BEST_MODEL_MAX_ASYNC: usize = 16;

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// `entity_extract_max_gleaning` (`lightrag.py:766`); a positive value runs
    /// exactly one extra round (`operate.py:4252-4253`).
    pub max_gleaning: usize,
    pub best_model_max_async: usize,
    /// Gleaning precheck budget (`MAX_EXTRACT_INPUT_TOKENS`, constants.py:38).
    pub max_extract_input_tokens: usize,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            max_gleaning: DEFAULT_MAX_GLEANING,
            best_model_max_async: DEFAULT_BEST_MODEL_MAX_ASYNC,
            max_extract_input_tokens: DEFAULT_MAX_EXTRACT_INPUT_TOKENS,
        }
    }
}

/// Run-fixed prompt strings (`operate.py:4027-4047`): the examples block and
/// the entity-type guidance are identical for every chunk of a run.
#[derive(Debug, Clone)]
pub struct ExtractPrompts {
    pub system: String,
    pub user_template: String,
    pub continue_template: String,
}

/// Build the three prompt strings once per extraction run.
pub fn build_extract_prompts() -> LlmResult<ExtractPrompts> {
    let examples = fill_template(
        EXTRACTION_EXAMPLES,
        &[
            ("tuple_delimiter", DEFAULT_TUPLE_DELIMITER),
            ("completion_delimiter", DEFAULT_COMPLETION_DELIMITER),
        ],
    )
    .map_err(|e| LlmError::Decode(format!("examples template: {e}")))?;
    let records = DEFAULT_MAX_EXTRACTION_RECORDS.to_string();
    let entities = DEFAULT_MAX_EXTRACTION_ENTITIES.to_string();
    let common = [
        ("tuple_delimiter", DEFAULT_TUPLE_DELIMITER),
        ("completion_delimiter", DEFAULT_COMPLETION_DELIMITER),
        ("entity_types_guidance", ENTITY_TYPES_GUIDANCE),
        ("examples", examples.as_str()),
        ("max_total_records", records.as_str()),
        ("max_entity_records", entities.as_str()),
        // No heading metadata flows into the chunk rows yet, so the optional
        // section-context block is always empty (`operate.py:4113-4118`).
        ("heading_context_block", ""),
    ];
    let mut with_language = common.to_vec();
    with_language.push(("language", "English"));

    let system = fill_template(ENTITY_EXTRACTION_SYSTEM, &with_language)
        .map_err(|e| LlmError::Decode(format!("system template: {e}")))?;
    let user_template = fill_template(ENTITY_EXTRACTION_USER, &with_language)
        .map_err(|e| LlmError::Decode(format!("user template: {e}")))?;
    let continue_template = fill_template(ENTITY_CONTINUE_EXTRACTION, &with_language)
        .map_err(|e| LlmError::Decode(format!("continue template: {e}")))?;

    Ok(ExtractPrompts {
        system,
        user_template,
        continue_template,
    })
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
/// (`_process_extraction_result`, operate.py:1508-1640).
///
/// The reference splits on newlines (not `##`), repairs rows the model merged
/// with the tuple delimiter, and recovers relation rows the model mis-prefixed
/// as `entity` (`_normalize_text_extraction_record_attributes`, :854).
pub fn parse_extraction_result(
    final_result: &str,
    chunk_key: &str,
    file_path: &str,
) -> ExtractedRecords {
    let completion = DEFAULT_COMPLETION_DELIMITER.to_lowercase();
    let records = split_string_by_multi_markers(
        final_result,
        &["\n", DEFAULT_COMPLETION_DELIMITER, completion.as_str()],
    );

    let mut extracted = ExtractedRecords::default();
    for record in &records {
        for row in repair_merged_rows(record) {
            let attributes = split_string_by_multi_markers(&row, &[DEFAULT_TUPLE_DELIMITER]);
            let attributes = normalize_record_attributes(attributes);
            if let Some(entity) =
                handle_single_entity_extraction(&attributes, chunk_key, file_path, 0)
            {
                extracted.push_node(entity.entity_name.clone(), entity);
                continue;
            }
            if let Some(relation) =
                handle_single_relationship_extraction(&attributes, chunk_key, file_path, 0)
            {
                let key = if relation.src_id <= relation.tgt_id {
                    (relation.src_id.clone(), relation.tgt_id.clone())
                } else {
                    (relation.tgt_id.clone(), relation.src_id.clone())
                };
                extracted.push_edge(key, relation);
            }
        }
    }
    extracted
}

/// The reference's LLM-output repair pass (`operate.py:1550-1579`): a record
/// packing several rows is re-split on the tuple-delimited `entity` /
/// `relation` markers, and a fragment that lost its prefix regains it.
fn repair_merged_rows(record: &str) -> Vec<String> {
    let tuple = DEFAULT_TUPLE_DELIMITER;
    let entity_marker = format!("{tuple}entity{tuple}");
    let relation_markers = [
        format!("{tuple}relationship{tuple}"),
        format!("{tuple}relation{tuple}"),
    ];
    let mut rows = Vec::new();
    for entity_row in split_string_by_multi_markers(record, &[entity_marker.as_str()]) {
        let entity_row = if entity_row.starts_with("entity") || entity_row.starts_with("relation") {
            entity_row
        } else {
            format!("entity{tuple}{entity_row}")
        };
        for relation_row in split_string_by_multi_markers(
            &entity_row,
            &[relation_markers[0].as_str(), relation_markers[1].as_str()],
        ) {
            if relation_row.starts_with("entity") || relation_row.starts_with("relation") {
                rows.push(relation_row);
            } else {
                rows.push(format!("relation{tuple}{relation_row}"));
            }
        }
    }
    rows
}

/// `_normalize_text_extraction_record_attributes` (`operate.py:854`): a
/// 5-field row prefixed `entity` is a mis-prefixed relation.
fn normalize_record_attributes(mut attributes: Vec<String>) -> Vec<String> {
    if attributes.len() == 5 {
        let prefix = attributes[0].trim().to_lowercase();
        if prefix.contains("entity") && !prefix.contains("relation") {
            attributes[0] = "relation".to_string();
        }
    }
    attributes
}

/// `_handle_single_relationship_extraction` (LightRAG `operate.py:774-837`):
/// the tuple is exactly 5 fields — `("relation", src, tgt, keywords,
/// description)` — and the weight is a fixed 1.0.
pub fn handle_single_relationship_extraction(
    attributes: &[String],
    chunk_key: &str,
    file_path: &str,
    timestamp: i64,
) -> Option<RelationRecord> {
    if attributes.len() != 5 || !attributes[0].contains("relation") {
        return None;
    }
    let src_id = normalize_entity_name(&attributes[1]);
    let tgt_id = normalize_entity_name(&attributes[2]);
    if src_id.is_empty() || tgt_id.is_empty() || src_id == tgt_id {
        return None;
    }
    let keywords = sanitize_and_normalize_extracted_text(&attributes[3], true).replace('，', ",");
    let description = sanitize_and_normalize_extracted_text(&attributes[4], false);
    if description.trim().is_empty() {
        return None;
    }
    Some(RelationRecord {
        src_id,
        tgt_id,
        weight: 1.0,
        description,
        source_id: chunk_key.to_string(),
        order: 1,
        keywords,
        file_path: file_path.to_string(),
        timestamp,
    })
}

/// `_handle_single_entity_extraction` (LightRAG `operate.py:710-768`):
/// the tuple is exactly 4 fields — `("entity", name, type, description)`.
pub fn handle_single_entity_extraction(
    attributes: &[String],
    chunk_key: &str,
    file_path: &str,
    timestamp: i64,
) -> Option<EntityRecord> {
    if attributes.len() != 4 || !attributes[0].contains("entity") {
        return None;
    }
    let entity_name = normalize_entity_name(&attributes[1]);
    if entity_name.is_empty() {
        return None;
    }
    let entity_type =
        normalize_entity_type(&sanitize_and_normalize_extracted_text(&attributes[2], true))?;
    Some(EntityRecord {
        entity_name,
        entity_type,
        description: sanitize_and_normalize_extracted_text(&attributes[3], false),
        source_id: chunk_key.to_string(),
        file_path: file_path.to_string(),
        timestamp,
    })
}

/// `_normalize_and_validate_entity_type` (`operate.py:662-711`): structural
/// characters, an all-empty comma split, and the reserved JavaScript prototype
/// names drop the record; otherwise the first comma token wins, spaces are
/// removed and the type is lowercased.
fn normalize_entity_type(raw: &str) -> Option<String> {
    if raw.trim().is_empty()
        || raw
            .chars()
            .any(|c| matches!(c, '\'' | '(' | ')' | '<' | '>' | '|' | '/' | '\\'))
    {
        return None;
    }
    let entity_type = match raw.split_once(',') {
        Some((first, _)) if first.trim().is_empty() => {
            let tokens: Vec<&str> = raw
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            if tokens.is_empty() {
                return None;
            }
            tokens[0].to_string()
        }
        Some((first, _)) => first.trim().to_string(),
        None => raw.to_string(),
    };
    let entity_type = entity_type.replace(' ', "").to_lowercase();
    if matches!(
        entity_type.as_str(),
        "__proto__" | "constructor" | "prototype"
    ) {
        return None;
    }
    Some(entity_type)
}

/// Everything one chunk task needs. The run-fixed strings (`ExtractPrompts`)
/// are cloned per task so each task is self-contained, and the field set keeps
/// the call site at a readable size.
struct ChunkTask {
    chunk_key: String,
    content: String,
    file_path: String,
    llm: CachedLlm,
    limiter: Limiter,
    options: ExtractOptions,
    prompts: ExtractPrompts,
    tokenizer: Tokenizer,
}

impl ChunkTask {
    /// One chunk: initial extraction, one gleaning round (guarded by the input
    /// token budget), then parsing (`operate.py:3990-4330`).
    async fn run(&self) -> LlmResult<ExtractedRecords> {
        let user_prompt = fill_template(
            &self.prompts.user_template,
            &[("input_text", &self.content)],
        )
        .map_err(|e| LlmError::Decode(format!("user template: {e}")))?;

        let (mut final_result, _) = self
            .limiter
            .run(self.llm.complete_cached_keyed(
                &user_prompt,
                Some(&self.prompts.system),
                &[],
                &ModelOptions::default(),
                "extract",
            ))
            .await?;
        let history = vec![
            ChatMessage::user(user_prompt),
            ChatMessage::assistant(final_result.clone()),
        ];

        // The reference runs exactly one gleaning round, and only when system +
        // history + continue prompt fits the input budget (`operate.py:4252-4286`).
        if self.options.max_gleaning > 0 && self.options.max_extract_input_tokens > 0 {
            let gleaning_tokens = self.tokenizer.token_len(&self.prompts.system)
                + self.tokenizer.token_len(&self.prompts.continue_template)
                + history
                    .iter()
                    .map(|message| self.tokenizer.token_len(&message.content))
                    .sum::<usize>();
            if gleaning_tokens > self.options.max_extract_input_tokens {
                tracing::warn!(
                    chunk = self.chunk_key,
                    gleaning_tokens,
                    limit = self.options.max_extract_input_tokens,
                    "gleaning skipped: input exceeds the extract budget"
                );
            } else {
                let (glean_result, _) = self
                    .limiter
                    .run(self.llm.complete_cached_keyed(
                        &self.prompts.continue_template,
                        Some(&self.prompts.system),
                        &history,
                        &ModelOptions::default(),
                        "extract",
                    ))
                    .await?;
                final_result.push_str(&glean_result);
            }
        }

        Ok(parse_extraction_result(
            &final_result,
            &self.chunk_key,
            &self.file_path,
        ))
    }
}

/// Run extraction over a chunk set with the reference's one-round gleaning,
/// concurrently, capped by [`ExtractOptions::best_model_max_async`].
pub async fn extract_entities(
    chunks: &[(String, Chunk)],
    llm: &CachedLlm,
    options: &ExtractOptions,
    tokenizer: &Tokenizer,
) -> LlmResult<ExtractedRecords> {
    let prompts = build_extract_prompts()?;
    let limiter = Limiter::new(options.best_model_max_async);

    let mut set: JoinSet<(usize, LlmResult<ExtractedRecords>)> = JoinSet::new();
    for (index, (chunk_key, chunk)) in chunks.iter().enumerate() {
        let task = ChunkTask {
            chunk_key: chunk_key.clone(),
            content: chunk.content.clone(),
            file_path: chunk.file_path.clone(),
            llm: llm.clone(),
            limiter: limiter.clone(),
            options: options.clone(),
            prompts: prompts.clone(),
            tokenizer: tokenizer.clone(),
        };
        set.spawn(async move { (index, task.run().await) });
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
