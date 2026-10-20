//! Config → durable [`Pipeline`]. One async call, every backend wired.

use std::sync::Arc;

use crate::config::Config;
use crate::core::traits::{Embedder, KvStore, VectorStore};
use crate::llm::cache::CachedLlm;
use crate::llm::fastembed::FastembedEmbedder;
use crate::llm::LlmClient;
use crate::pipeline::{CommunityMode, Pipeline, PipelineOptions};
use crate::store::lancedb::LanceVector;
use crate::store::redb_graph::RedbGraph;
use crate::store::repair::RepairQueue;
use crate::store::turso::TursoKv;

/// What one workspace's stores resolve to.
pub struct Assembled {
    pub pipeline: Pipeline,
    pub truth_path: std::path::PathBuf,
    /// The derived graph index. Droppable: rebuild it from the truth layer.
    pub graph_path: std::path::PathBuf,
    pub vector_uri: String,
    pub embedder: Arc<dyn Embedder>,
}

/// Open one KV namespace in the truth file.
async fn kv(path: &std::path::Path, namespace: &str) -> Result<Arc<dyn KvStore>, String> {
    let store = TursoKv::open(path.to_string_lossy().as_ref(), namespace)
        .await
        .map_err(|error| format!("opening {namespace}: {error}"))?;
    Ok(Arc::new(store) as Arc<dyn KvStore>)
}

/// Open one vector table in the lancedb directory, sharing the embedder.
///
/// Table names allow only `[A-Za-z0-9_.-]`, so the namespace prefix is
/// sanitized (`:` → `_`) rather than rejected: the turso side keeps the
/// colon form, this side keeps a valid name.
async fn open(
    uri: &str,
    namespace: String,
    embedder: Arc<dyn Embedder>,
    cosine: f32,
) -> Result<Arc<dyn VectorStore>, String> {
    let table = namespace.replace(':', "_");
    let store = LanceVector::open(uri, table, embedder, cosine)
        .await
        .map_err(|error| format!("opening vector store: {error}"))?;
    Ok(Arc::new(store) as Arc<dyn VectorStore>)
}

impl Config {
    /// Build the durable pipeline this config describes: one turso file for
    /// truth, one redb file for the graph adjacency, one lancedb directory for
    /// the vector tables, the configured chat model wrapped in the response
    /// cache, and the local ONNX embedder.
    pub async fn assemble(&self) -> Result<Assembled, String> {
        let dir = &self.store.data_dir;
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("creating {}: {error}", dir.display()))?;
        let truth_path = dir.join("truth.db");
        let graph_path = dir.join("graph.redb");
        let vector_uri = dir.join("vectors").to_string_lossy().to_string();

        let embedder: Arc<dyn Embedder> = Arc::new(
            FastembedEmbedder::new(&self.embedding.model)
                .map_err(|error| format!("loading embedding model: {error}"))?,
        );

        // KV namespaces are prefixed per workspace namespace so two configs
        // sharing one truth file stay isolated (the reference keeps one JSON
        // file per namespace; we keep one table with a prefix).
        let ns = &self.store.namespace;
        let full_docs = kv(&truth_path, &format!("{ns}:full_docs")).await?;
        let text_chunks = kv(&truth_path, &format!("{ns}:text_chunks")).await?;
        let community_reports = kv(&truth_path, &format!("{ns}:community_reports")).await?;
        let entity_chunks = kv(&truth_path, &format!("{ns}:entity_chunks")).await?;
        let relation_chunks = kv(&truth_path, &format!("{ns}:relation_chunks")).await?;
        let full_entities = kv(&truth_path, &format!("{ns}:full_entities")).await?;
        let full_relations = kv(&truth_path, &format!("{ns}:full_relations")).await?;
        let cache_kv = kv(&truth_path, &format!("{ns}:llm_response_cache")).await?;
        let repair_kv = kv(&truth_path, &format!("{ns}:repair_queue")).await?;

        let graph = Arc::new(
            RedbGraph::open(&graph_path, &self.store.graph_scope)
                .await
                .map_err(|error| format!("opening graph: {error}"))?,
        );

        let cosine = self.query.cosine_better_than_threshold;
        let entities_vdb = open(
            &vector_uri,
            format!("{ns}:entities_vdb"),
            embedder.clone(),
            cosine,
        )
        .await?;
        let relationships_vdb = open(
            &vector_uri,
            format!("{ns}:relationships_vdb"),
            embedder.clone(),
            cosine,
        )
        .await?;
        let chunks_vdb = if self.pipeline.enable_naive_rag {
            Some(
                open(
                    &vector_uri,
                    format!("{ns}:text_chunks"),
                    embedder.clone(),
                    cosine,
                )
                .await?,
            )
        } else {
            None
        };

        let chat: Arc<dyn LlmClient> = Arc::new(
            self.llm
                .client()
                .map_err(|error| format!("llm client: {error}"))?,
        );
        let llm = CachedLlm::new(chat, cache_kv)
            .with_enable_llm_cache(self.pipeline.enable_llm_cache)
            .with_enable_llm_cache_for_extract(self.pipeline.enable_llm_cache_for_extract);

        let pipeline = Pipeline {
            full_docs,
            text_chunks,
            community_reports,
            graph,
            entities_vdb,
            relationships_vdb: Some(relationships_vdb),
            chunks_vdb,
            tracking: crate::index::tracking::TrackingStores {
                entity_chunks,
                relation_chunks,
            },
            full_entities,
            full_relations,
            llm,
            tokenizer: crate::core::text::Tokenizer::for_gpt_4o()
                .map_err(|error| format!("tokenizer: {error}"))?,
            repair: Arc::new(RepairQueue::new(repair_kv)),
            options: PipelineOptions {
                chunk_token_size: self.pipeline.chunk_token_size,
                chunk_overlap_token_size: self.pipeline.chunk_overlap_token_size,
                enable_naive_rag: self.pipeline.enable_naive_rag,
                enable_local: self.pipeline.enable_local,
                best_model_max_async: self.pipeline.best_model_max_async,
                embedding_batch_num: self.pipeline.embedding_batch_num,
                community_mode: self.pipeline.community_mode()?,
                ..PipelineOptions::default()
            },
        };

        Ok(Assembled {
            pipeline,
            truth_path,
            graph_path,
            vector_uri,
            embedder,
        })
    }
}

/// Kept for callers that want the chat client type without reaching into the
/// llm module.
pub type ChatClient = crate::llm::openai::OpenAiClient;

/// The community mode the config selects, surfaced for reporting.
pub type Communities = CommunityMode;
