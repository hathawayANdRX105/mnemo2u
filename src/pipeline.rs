//! Insert/query pipeline — port of the reference `GraphRAG` facade
//! (`graphrag.py::ainsert` 279-347, `aquery` 236-275).
//!
//! Commit order (R1): derived stores (graph, vector) are written during the
//! run with idempotent upserts; the **`full_docs` / `text_chunks` rows commit
//! last** and are the commit point. A crash before that leaves the document
//! "not ingested", so a retry re-runs extraction (LLM cache makes it cheap)
//! and re-upserts the derived rows — the reference's own retry semantics.
//!
//! Derived writes (graph, vectors, reports) additionally go through a
//! [`RepairQueue`]: when one fails mid-insert the ingest still finishes, the
//! failed payload is queued in the truth layer, and the next [`Pipeline::flush`]
//! replays it. Truth-first commits plus a replay queue means no ingest is lost
//! to a transient derived-store error, and no derived row is silently skipped.

use tracing::warn;

use std::sync::Arc;

use crate::core::rag::{Chunk, DocRecord, EntityRecord, QueryMode, QueryParam, RelationRecord};
use crate::core::text::{compute_mdhash_id, Tokenizer};
use crate::core::traits::{
    GraphStore, KvStore, Result as StoreResult, StoreError, VectorRow, VectorStore,
};
use crate::graph::chunk::{get_chunks, DEFAULT_CHUNK_OVERLAP_TOKEN_SIZE, DEFAULT_CHUNK_TOKEN_SIZE};
use crate::graph::community::{detect_communities, CommunityOptions};
use crate::graph::extract::{extract_entities, ExtractOptions};
use crate::graph::merge::{merge_edges_then_upsert, merge_nodes_then_upsert, MergeOptions};
use crate::graph::reports::{generate_community_report, ReportOptions};
use crate::llm::cache::CachedLlm;
use crate::llm::LlmResult;
use crate::query::QueryStores;
use crate::store::repair::{RepairItem, RepairQueue, RepairTarget};

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
    /// R2 T8: community report policy. `off` (default) never runs detection
    /// or reports; `on_demand` rebuilds them only when asked — LightRAG has no
    /// community detection at all, so the incremental write path must not pay
    /// for a global recompute on every insert.
    pub community_mode: CommunityMode,
    /// `enable_local` (`graphrag.py:58`, default true). False skips entity
    /// embeddings entirely (`entities_vdb` is not built) and makes local queries
    /// fail like the reference's `aquery` guard.
    pub enable_local: bool,
    pub best_model_max_async: usize,
    pub embedding_batch_num: usize,
    /// R2: source-id cap on the tracking rows (`utils.py:7244`).
    pub source_ids_limit: usize,
    /// R2: `KEEP` (oldest) or `FIFO` (newest) cap strategy.
    pub source_ids_limit_method: String,
    /// R2 merge gates (`constants.py:30,32,34,36`).
    pub index_merge: crate::index::merge::IndexMergeOptions,
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
            community_mode: CommunityMode::Off,
            enable_local: true,
            best_model_max_async: 16,
            embedding_batch_num: 32,
            source_ids_limit: 300,
            source_ids_limit_method: crate::index::tracking::DEFAULT_SOURCE_IDS_LIMIT_METHOD
                .to_string(),
            index_merge: crate::index::merge::IndexMergeOptions::default(),
        }
    }
}

/// Community report policy (`communities.mode`, stage doc §0 decision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityMode {
    /// Never detect or report communities during insert.
    Off,
    /// Rebuild community reports from the current graph when asked.
    OnDemand,
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
    /// `relationships_vdb` — the high-level keyword arm (R2).
    pub relationships_vdb: Option<Arc<dyn VectorStore>>,
    pub chunks_vdb: Option<Arc<dyn VectorStore>>,
    /// R2 tracking rows: entity → chunk ids and relation key → chunk ids.
    pub tracking: crate::index::tracking::TrackingStores,
    /// R2 Phase-0 anchors: document → mentioned entity / relation keys.
    pub full_entities: Arc<dyn KvStore>,
    pub full_relations: Arc<dyn KvStore>,
    pub llm: CachedLlm,
    pub tokenizer: Tokenizer,
    pub options: PipelineOptions,
    /// Derived-write failures, queued in the truth layer and replayed by `flush`.
    pub repair: Arc<RepairQueue>,
}

impl Pipeline {
    /// Query-side view of the same stores (`aquery`).
    pub fn query_stores(&self) -> QueryStores {
        QueryStores {
            graph: self.graph.clone(),
            entities_vdb: self.entities_vdb.clone(),
            relationships_vdb: self.relationships_vdb.clone(),
            chunks_vdb: self.chunks_vdb.clone(),
            community_reports: self.community_reports.clone(),
            text_chunks: self.text_chunks.clone(),
            llm: self.llm.clone(),
            tokenizer: self.tokenizer.clone(),
            max_async: self.options.best_model_max_async,
        }
    }

    pub async fn query(&self, query: &str, param: &QueryParam) -> LlmResult<String> {
        // Mirrors the reference's `aquery` guard (`graphrag.py:237-240`).
        if param.mode == QueryMode::Local && !self.options.enable_local {
            return Err(crate::llm::LlmError::Transport(
                "enable_local is False, cannot query in local mode".to_string(),
            ));
        }
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

        // The provenance path every extraction record carries. We ingest raw
        // strings rather than files, so the document key is the path — better
        // provenance than the reference's `unknown_source` default.
        let doc_pairs: Vec<(String, String)> = new_docs
            .iter()
            .map(|(key, _)| (key.clone(), key.clone()))
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
                let rows: Vec<VectorRow> = chunks
                    .iter()
                    .map(|(key, chunk)| VectorRow {
                        id: key.clone(),
                        content: chunk.content.clone(),
                        meta: serde_json::to_value(chunk).expect("chunk serializes"),
                    })
                    .collect();
                for batch in rows.chunks(self.options.embedding_batch_num.max(1)) {
                    if let Err(error) = chunks_vdb.upsert(batch.to_vec()).await {
                        // Naive RAG is an optional arm; a failure here defers to
                        // the repair queue instead of aborting the ingest.
                        self.queue_vectors(RepairTarget::ChunkVector, batch).await?;
                        warn!(%error, rows = batch.len(), "chunk vector write deferred to repair");
                    }
                }
            }
        }

        let records = extract_entities(&chunks, &self.llm, &self.options.extract, &self.tokenizer)
            .await
            .map_err(|e| StoreError::Backend(format!("extraction: {e}")))?;
        if records.nodes.is_empty() && records.edges.is_empty() {
            return Ok(InsertOutcome::NoEntities);
        }

        // R2 Phase 0: the anchor rows say which graph elements this document
        // owns, so a crash mid-run is detectable and a delete can find its
        // candidates without scanning every tracking row.
        let (scope_entities, scope_relations) = crate::index::purge::document_scope_keys(&records);
        let anchor = crate::index::merge::ScopeAnchor {
            entities: scope_entities,
            relations: scope_relations,
        };
        for (key, _) in &new_docs {
            if let Err(error) = crate::index::merge::write_scope_anchor(
                self.full_entities.as_ref(),
                self.full_relations.as_ref(),
                key,
                &anchor,
            )
            .await
            {
                warn!(%error, doc = key, "scope anchor write deferred to repair");
                self.repair
                    .record(
                        RepairTarget::EntityChunk,
                        key,
                        serde_json::to_value(&anchor).expect("anchor serializes"),
                    )
                    .await?;
            }
        }

        // R2 Phase 1/2: per-element incremental merge. A failure queues the
        // element for repair and the ingest continues.
        let mut merged_nodes = Vec::new();
        for (entity_name, nodes_data) in &records.nodes {
            match crate::index::merge::merge_node(
                entity_name,
                nodes_data,
                self.graph.as_ref(),
                &self.llm,
                &self.tokenizer,
                &self.options.index_merge,
            )
            .await
            {
                Ok(node) => merged_nodes.push(node),
                Err(error) => {
                    self.queue_graph_node(entity_name, nodes_data).await?;
                    warn!(%error, entity = entity_name, "graph node write deferred to repair");
                }
            }
        }
        let mut relation_rows: Vec<crate::core::traits::VectorRow> = Vec::new();
        for ((src, tgt), edges_data) in &records.edges {
            match crate::index::merge::merge_edge(
                src,
                tgt,
                edges_data,
                self.graph.as_ref(),
                &self.llm,
                &self.tokenizer,
                &self.options.index_merge,
            )
            .await
            {
                Ok(edge) => {
                    let source_id = edges_data
                        .first()
                        .map(|record| record.source_id.clone())
                        .unwrap_or_default();
                    relation_rows.extend(crate::index::merge::relation_vector_rows(
                        src, tgt, &edge, &source_id,
                    ));
                }
                Err(error) => {
                    self.queue_graph_edge(src, tgt, edges_data).await?;
                    warn!(%error, src, tgt, "graph edge write deferred to repair");
                }
            }
        }

        // R2 tracking rows (entity_chunks / relation_chunks).
        let tracking_rows = crate::index::merge::tracking_rows(&records);
        crate::index::merge::write_tracking_rows(
            &self.tracking,
            &tracking_rows,
            self.options.source_ids_limit,
            &self.options.source_ids_limit_method,
            &self.repair,
        )
        .await?;

        // The reference only builds `entities_vdb` when `enable_local` is set
        // (`graphrag.py:206-214`), so with it off no entity is ever embedded.
        if self.options.enable_local {
            let vector_rows = crate::index::merge::entity_vector_rows(&merged_nodes);
            crate::index::merge::upsert_entity_vectors(
                self.entities_vdb.as_ref(),
                &vector_rows,
                self.options.embedding_batch_num,
                &self.repair,
            )
            .await?;
        }

        if let Some(relationships_vdb) = &self.relationships_vdb {
            crate::index::merge::upsert_relation_vectors(
                relationships_vdb.as_ref(),
                &relation_rows,
                self.options.embedding_batch_num,
                &self.repair,
            )
            .await?;
        }

        // R2 T8: community reports are off by default. `on_demand` rebuilds
        // them from the graph when explicitly asked; `off` never spends the
        // LLM calls, and neither mode touches the insert path.
        if self.options.community_mode == CommunityMode::OnDemand {
            self.rebuild_community_reports().await?;
        }

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

    /// `on_demand` community rebuild — the R1 detection + report pass, run only
    /// when the policy asks for it (never from the insert path).
    pub async fn rebuild_community_reports(&self) -> StoreResult<()> {
        self.community_reports.drop_all().await?;
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
        if let Err(error) = self
            .community_reports
            .upsert(crate::graph::reports::report_kv_rows(&reports))
            .await
        {
            self.repair
                .record(
                    RepairTarget::CommunityReport,
                    "all",
                    serde_json::to_value(&reports).expect("report rows serialize"),
                )
                .await?;
            warn!(%error, "community report write deferred to repair");
        }
        Ok(())
    }

    /// The reference closes a run by flushing every store
    /// (`_insert_done`, graphrag.py:349-372).
    pub async fn flush(&self) -> StoreResult<()> {
        self.drain_repair().await?;
        self.community_reports.index_done().await?;
        self.entities_vdb.index_done().await?;
        if let Some(chunks_vdb) = &self.chunks_vdb {
            chunks_vdb.index_done().await?;
        }
        self.graph.index_done().await?;
        Ok(())
    }

    /// Rebuild the chunk vector index from the truth layer.
    ///
    /// `text_chunks` owns the chunk rows, so the naive-RAG index is fully
    /// derivable: deleting the derived table and re-running this restores
    /// identical retrieval (the commit protocol's "rebuildable derived" rule).
    /// Returns the number of rows written.
    pub async fn rebuild_chunk_vectors(&self) -> StoreResult<usize> {
        let chunks_vdb = self
            .chunks_vdb
            .as_ref()
            .ok_or_else(|| StoreError::NotFound("naive RAG disabled".into()))?;
        let keys = self.text_chunks.all_keys().await?;
        let rows = self.text_chunks.get_by_ids(&keys).await?;
        let mut vector_rows = Vec::with_capacity(rows.len());
        for (key, row) in keys.into_iter().zip(rows) {
            let Some(row) = row else { continue };
            let content = row
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            vector_rows.push(VectorRow {
                id: key,
                content,
                meta: row,
            });
        }
        let written = vector_rows.len();
        chunks_vdb.upsert(vector_rows).await?;
        chunks_vdb.index_done().await?;
        Ok(written)
    }

    /// Replay queued derived writes (T11). A replay that fails again stays in
    /// the queue: the next `flush` retries it, so a persistent failure is
    /// visible as a non-empty queue instead of dropping the row.
    async fn drain_repair(&self) -> StoreResult<()> {
        for item in self.repair.pending().await? {
            match self.replay(&item).await {
                Ok(()) => self.repair.done(std::slice::from_ref(&item.id)).await?,
                Err(error) => {
                    warn!(%error, id = item.id, target = ?item.target, "repair replay deferred");
                }
            }
        }
        Ok(())
    }

    async fn replay(&self, item: &RepairItem) -> StoreResult<()> {
        match item.target {
            RepairTarget::GraphNode => {
                let records: Vec<EntityRecord> = serde_json::from_value(item.payload.clone())
                    .map_err(|error| {
                        StoreError::Backend(format!("repair graph node payload: {error}"))
                    })?;
                let entity_name = records
                    .first()
                    .map(|record| record.entity_name.clone())
                    .unwrap_or_default();
                merge_nodes_then_upsert(
                    &entity_name,
                    &records,
                    self.graph.as_ref(),
                    &self.llm,
                    &self.tokenizer,
                    &self.options.merge,
                )
                .await
                .map_err(|error| StoreError::Backend(format!("repair graph node: {error}")))?;
            }
            RepairTarget::GraphEdge => {
                let records: Vec<RelationRecord> = serde_json::from_value(item.payload.clone())
                    .map_err(|error| {
                        StoreError::Backend(format!("repair graph edge payload: {error}"))
                    })?;
                let (src, tgt) = match records.first() {
                    Some(record) => (record.src_id.clone(), record.tgt_id.clone()),
                    None => return Ok(()),
                };
                merge_edges_then_upsert(
                    &src,
                    &tgt,
                    &records,
                    self.graph.as_ref(),
                    &self.llm,
                    &self.tokenizer,
                    &self.options.merge,
                )
                .await
                .map_err(|error| StoreError::Backend(format!("repair graph edge: {error}")))?;
            }
            RepairTarget::ChunkVector => {
                let rows: Vec<VectorRow> =
                    serde_json::from_value(item.payload.clone()).map_err(|error| {
                        StoreError::Backend(format!("repair vector payload: {error}"))
                    })?;
                let Some(store) = &self.chunks_vdb else {
                    return Err(StoreError::NotFound("chunks_vdb disabled".into()));
                };
                store.upsert(rows).await?;
            }
            RepairTarget::EntityVector => {
                let rows: Vec<VectorRow> =
                    serde_json::from_value(item.payload.clone()).map_err(|error| {
                        StoreError::Backend(format!("repair vector payload: {error}"))
                    })?;
                self.entities_vdb.upsert(rows).await?;
            }
            RepairTarget::CommunityReport => {
                let rows: Vec<(String, serde_json::Value)> =
                    serde_json::from_value(item.payload.clone()).map_err(|error| {
                        StoreError::Backend(format!("repair report payload: {error}"))
                    })?;
                self.community_reports.upsert(rows).await?;
            }
            RepairTarget::RelationVector => {
                let rows: Vec<VectorRow> =
                    serde_json::from_value(item.payload.clone()).map_err(|error| {
                        StoreError::Backend(format!("repair relation vector payload: {error}"))
                    })?;
                let store = self
                    .relationships_vdb
                    .as_ref()
                    .ok_or_else(|| StoreError::NotFound("relationships_vdb disabled".into()))?;
                store.upsert(rows).await?;
            }
            RepairTarget::EntityChunk => {
                let row: serde_json::Value = item.payload.clone();
                self.tracking
                    .entity_chunks
                    .upsert(vec![(Self::repair_row_key(item), row)])
                    .await?;
            }
            RepairTarget::RelationChunk => {
                let row: serde_json::Value = item.payload.clone();
                self.tracking
                    .relation_chunks
                    .upsert(vec![(Self::repair_row_key(item), row)])
                    .await?;
            }
        }
        Ok(())
    }

    /// Repair ids are `target:<name>`; the row payload carries its own key, so the
    /// tracker row replays against the key it was queued under.
    fn repair_row_key(item: &RepairItem) -> String {
        item.id
            .split_once(':')
            .map(|(_, name)| name.to_string())
            .unwrap_or_else(|| item.id.clone())
    }

    async fn queue_graph_node(
        &self,
        entity_name: &str,
        records: &[EntityRecord],
    ) -> StoreResult<()> {
        self.repair
            .record(
                RepairTarget::GraphNode,
                entity_name,
                serde_json::to_value(records).expect("records serialize"),
            )
            .await
    }

    async fn queue_graph_edge(
        &self,
        src: &str,
        tgt: &str,
        records: &[RelationRecord],
    ) -> StoreResult<()> {
        let name = format!("{src}{tgt}");
        self.repair
            .record(
                RepairTarget::GraphEdge,
                &name,
                serde_json::to_value(records).expect("records serialize"),
            )
            .await
    }

    async fn queue_vectors(&self, target: RepairTarget, rows: &[VectorRow]) -> StoreResult<()> {
        for row in rows {
            self.repair
                .record(
                    target,
                    &row.id,
                    serde_json::to_value(std::slice::from_ref(row)).expect("row serializes"),
                )
                .await?;
        }
        Ok(())
    }
}
