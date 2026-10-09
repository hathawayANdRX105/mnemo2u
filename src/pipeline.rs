//! Insert/query pipeline — port of the reference `GraphRAG` facade
//! (`graphrag.py::ainsert` 279-347, `aquery` 236-275).
//!
//! Commit order (R1): derived stores (graph, vector) are written during the
//! run with idempotent upserts; the **`full_docs` / `text_chunks` rows commit
//! last** and are the commit point. A crash before that leaves the document
//! "not ingested", so a retry re-runs extraction (LLM cache makes it cheap)
//! and re-upserts the derived rows — the reference's own retry semantics.

use std::sync::Arc;

use crate::core::rag::{Chunk, DocRecord, QueryParam};
use crate::core::text::{compute_mdhash_id, Tokenizer};
use crate::core::traits::{GraphStore, KvStore, Result as StoreResult, StoreError, VectorStore};
use crate::graph::chunk::{get_chunks, DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE, DEFAULT_CHUNK_TOKEN_SIZE};
use crate::graph::community::{detect_communities, CommunityOptions};
use crate::graph::extract::{extract_entities, ExtractOptions};
use crate::graph::merge::{
    entity_vector_rows, merge_edges_then_upsert, merge_nodes_then_upsert, MergeOptions,
};
use crate::graph::reports::{generate_community_report, report_kv_rows, ReportOptions};
use crate::llm::cache::CachedLlm;
use crate::llm::LlmResult;
use crate::query::QueryStores;

/// Namespaces mirroring the reference KV stores (`graphrag.py:186-205`).
pub const NS_FULL_DOCS: &str = "full_docs";
pub const NS_TEXT_CHUNKS: &str = "text_chunks";
pub const NS_LLM_CACHE: &str = "llm_response_cache";
pub const NS_COMMUNITY_REPORTS: &str = "community_reports";

#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub chunk_token_size: usize,
    pub chunk_overlap_token_size: usize,
    pub extract: ExtractOptions,
    pub merge: MergeOptions,
    pub community: CommunityOptions,
    pub report: ReportOptions,
    /// `enable_naive_rag` (`graphrag.py:74`, default false).
    pub enable_naive_rag: bool,
    pub best_model_max_async: usize,
    pub embedding_batch_num: usize,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            chunk_token_size: DEFAULT_CHUNK_TOKEN_SIZE,
            chunk_overlap_token_size: DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE,
            extract: ExtractOptions::default(),
            merge: MergeOptions::default(),
            community: CommunityOptions::default(),
            report: ReportOptions::default(),
            enable_naive_rag: false,
            best_model_max_async: 16,
            embedding_batch_num: 32,
        }
    }
}

/// What one `insert` did (mirrors the reference's early-return branches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    /// All documents were already present (`full_docs.filter_keys`).
    AllDocsKnown,
    /// All chunks were already present (`text_chunks.filter_keys`).
    AllChunksKnown,
    /// No entities were extracted.
    NoEntities,
    Inserted {
        docs: usize,
        chunks: usize,
        entities: usize,
        relations: usize,
    },
}

pub struct Pipeline {
    pub full_docs: Arc<dyn KvStore>,
    pub text_chunks: Arc<dyn KvStore>,
    pub community_reports: Arc<dyn KvStore>,
    pub graph: Arc<dyn GraphStore>,
    pub entities_vdb: Arc<dyn VectorStore>,
    pub chunks_vdb: Option<Arc<dyn VectorStore>>,
    pub llm: CachedLlm,
    pub tokenizer: Tokenizer,
    pub options: PipelineOptions,
}

impl Pipeline {
    /// Query-side view of the same stores (`aquery`).
    pub fn query_stores(&self) -> QueryStores {
        QueryStores {
            graph: self.graph.clone(),
            entities_vdb: self.entities_vdb.clone(),
            chunks_vdb: self.chunks_vdb.clone(),
            community_reports: self.community_reports.clone(),
            text_chunks: self.text_chunks.clone(),
            llm: self.llm.clone(),
            tokenizer: self.tokenizer.clone(),
            max_async: self.options.best_model_max_async,
        }
    }

    pub async fn query(&self, query: &str, param: &QueryParam) -> LlmResult<String> {
        self.query_stores().query(query, param).await
    }

    /// `ainsert` (`graphrag.py:279-347`).
    pub async fn insert(&self, strings: Vec<String>) -> StoreResult<InsertOutcome> {
        let new_docs: Vec<(String, DocRecord)> = strings
            .into_iter()
            .map(|content| {
                let trimmed = content.trim().to_string();
                (
                    compute_mdhash_id(&trimmed, "doc-"),
                    DocRecord { content: trimmed },
                )
            })
            .collect();
        let doc_keys: Vec<String> = new_docs.iter().map(|(key, _)| key.clone()).collect();
        let unknown = self.full_docs.filter_keys(&doc_keys).await?;
        let new_docs: Vec<(String, DocRecord)> = new_docs
            .into_iter()
            .filter(|(key, _)| unknown.contains(key))
            .collect();
        if new_docs.is_empty() {
            return Ok(InsertOutcome::AllDocsKnown);
        }

        let doc_pairs: Vec<(String, String)> = new_docs
            .iter()
            .map(|(key, doc)| (key.clone(), doc.content.clone()))
            .collect();
        let chunks = get_chunks(
            &doc_pairs,
            &self.tokenizer,
            self.options.chunk_overlap_token_size,
            self.options.chunk_token_size,
        )
        .map_err(|e| StoreError::Backend(format!("chunking: {e}")))?;

        let chunk_keys: Vec<String> = chunks.iter().map(|(key, _)| key.clone()).collect();
        let unknown_chunks = self.text_chunks.filter_keys(&chunk_keys).await?;
        let chunks: Vec<(String, Chunk)> = chunks
            .into_iter()
            .filter(|(key, _)| unknown_chunks.contains(key))
            .collect();
        if chunks.is_empty() {
            return Ok(InsertOutcome::AllChunksKnown);
        }

        if self.options.enable_naive_rag {
            if let Some(chunks_vdb) = &self.chunks_vdb {
                chunks_vdb
                    .upsert(
                        chunks
                            .iter()
                            .map(|(key, chunk)| crate::core::traits::VectorRow {
                                id: key.clone(),
                                content: chunk.content.clone(),
                                meta: serde_json::to_value(chunk).expect("chunk serializes"),
                            })
                            .collect(),
                    )
                    .await?;
            }
        }

        // The reference drops every community report on each insert because
        // communities are not incrementally maintained (graphrag.py:329-330).
        self.community_reports.drop_all().await?;

        let records = extract_entities(&chunks, &self.llm, &self.options.extract)
            .await
            .map_err(|e| StoreError::Backend(format!("extraction: {e}")))?;
        if records.nodes.is_empty() {
            return Ok(InsertOutcome::NoEntities);
        }

        let mut merged_nodes = Vec::new();
        for (entity_name, nodes_data) in &records.nodes {
            let node = merge_nodes_then_upsert(
                entity_name,
                nodes_data,
                self.graph.as_ref(),
                &self.llm,
                &self.tokenizer,
                &self.options.merge,
            )
            .await
            .map_err(|e| StoreError::Backend(format!("merge node: {e}")))?;
            merged_nodes.push(node);
        }
        for ((src, tgt), edges_data) in &records.edges {
            merge_edges_then_upsert(
                src,
                tgt,
                edges_data,
                self.graph.as_ref(),
                &self.llm,
                &self.tokenizer,
                &self.options.merge,
            )
            .await
            .map_err(|e| StoreError::Backend(format!("merge edge: {e}")))?;
        }

        self.entities_vdb
            .upsert(entity_vector_rows(&merged_nodes))
            .await?;

        let communities = detect_communities(self.graph.as_ref(), &self.options.community).await?;
        let reports = generate_community_report(
            self.graph.as_ref(),
            &communities.schema,
            &self.llm,
            &self.tokenizer,
            &self.options.report,
            self.options.best_model_max_async,
        )
        .await
        .map_err(|e| StoreError::Backend(format!("community reports: {e}")))?;
        self.community_reports
            .upsert(report_kv_rows(&reports))
            .await?;

        // Commit point: documents and chunks become visible last.
        let doc_rows = new_docs
            .iter()
            .map(|(key, doc)| {
                (
                    key.clone(),
                    serde_json::to_value(doc).expect("doc serializes"),
                )
            })
            .collect();
        self.full_docs.upsert(doc_rows).await?;
        let chunk_rows: Vec<(String, serde_json::Value)> = chunks
            .iter()
            .map(|(key, chunk)| {
                (
                    key.clone(),
                    serde_json::to_value(chunk).expect("chunk serializes"),
                )
            })
            .collect();
        self.text_chunks.upsert(chunk_rows).await?;

        Ok(InsertOutcome::Inserted {
            docs: new_docs.len(),
            chunks: chunks.len(),
            entities: records.nodes.len(),
            relations: records.edges.len(),
        })
    }

    /// The reference closes a run by flushing every store
    /// (`_insert_done`, graphrag.py:349-372).
    pub async fn flush(&self) -> StoreResult<()> {
        self.community_reports.index_done().await?;
        self.entities_vdb.index_done().await?;
        if let Some(chunks_vdb) = &self.chunks_vdb {
            chunks_vdb.index_done().await?;
        }
        self.graph.index_done().await?;
        Ok(())
    }
}
