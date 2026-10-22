//! Incremental-write idempotence — audit gap N1 (stage doc R2 TAC1/FAC1).
//!
//! A canonical snapshot of every persisted row (KV truth tables, graph
//! node/edge rows, entity/relation/chunk vector rows, LLM call count) is
//! taken after the first insert; re-inserting the same documents must be a
//! byte-identical no-op with zero LLM delta, and inserting an unrelated
//! third document must leave every row owned by the first two documents'
//! exclusive elements byte-identical (FAC1 "unrelated rows unchanged").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use mnemo2u::compute_mdhash_id;
use mnemo2u::core::traits::{Embedder, GraphStore, KvStore, VectorHit, VectorRow, VectorStore};
use mnemo2u::index::tracking::TrackingStores;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::{MockEmbedder, RoutedLlm};
use mnemo2u::pipeline::{InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::memory::{MemoryGraph, MemoryKv, MemoryVector};
use mnemo2u::store::repair::RepairQueue;
use mnemo2u::StoreResult;
use mnemo2u::Tokenizer;

/// A and B share SOLSTICE; VESPERINE is owned by A only, ORLOVE by B only.
const DOC_A: &str =
    "VESPERINE parade: The VESPERINE troupe opened the SOLSTICE festival with lantern boats.";
const DOC_B: &str = "ORLOVE builds: ORLOVE began building light towers for the SOLSTICE festival.";
/// C shares SOLSTICE with A/B and brings its own exclusive GAMMA FIELD.
const DOC_C: &str = "GAMMA FIELD trial: The SOLSTICE panel approved the GAMMA FIELD trial budget.";

const EXTRACTION_A: &str = "entity<|#|>SOLSTICE<|#|>event<|#|>SOLSTICE is a summer festival.\n\
entity<|#|>VESPERINE<|#|>artifact<|#|>VESPERINE is a parade troupe.\n\
relation<|#|>SOLSTICE<|#|>VESPERINE<|#|>hosts<|#|>SOLSTICE hosts the VESPERINE parade.\n\
<|COMPLETE|>";
const EXTRACTION_B: &str = "entity<|#|>SOLSTICE<|#|>event<|#|>SOLSTICE is a winter festival.\n\
entity<|#|>ORLOVE<|#|>organization<|#|>ORLOVE builds light shows.\n\
relation<|#|>SOLSTICE<|#|>ORLOVE<|#|>sponsors<|#|>SOLSTICE sponsors ORLOVE.\n\
<|COMPLETE|>";
const EXTRACTION_C: &str = "entity<|#|>SOLSTICE<|#|>event<|#|>SOLSTICE researchers fund trials.\n\
entity<|#|>GAMMA FIELD<|#|>location<|#|>GAMMA FIELD is a trial site.\n\
relation<|#|>SOLSTICE<|#|>GAMMA FIELD<|#|>funds<|#|>SOLSTICE funds the GAMMA FIELD trial.\n\
<|COMPLETE|>";
const GLEANING_EMPTY: &str = "";

/// Per-document extraction responses are keyed on a phrase only the
/// document's own chunk text carries, so the routed mock answers each
/// chunk with its own corpus; the gleaning round (continue template plus
/// system, no chunk text) and the fallback share one rule each.
fn routing() -> Vec<(String, String)> {
    vec![
        ("VESPERINE parade".to_string(), EXTRACTION_A.to_string()),
        ("ORLOVE builds".to_string(), EXTRACTION_B.to_string()),
        ("GAMMA FIELD trial".to_string(), EXTRACTION_C.to_string()),
        (
            "missed or incorrectly formatted".to_string(),
            GLEANING_EMPTY.to_string(),
        ),
        (
            "Extract entities and relationships".to_string(),
            String::new(),
        ),
        ("high_level_keywords".to_string(), String::new()),
    ]
}

/// Vector store that mirrors its rows, so a test can enumerate ids and
/// content without an ANN query: retrieval semantics stay those of the
/// real `MemoryVector`, the mirror only records upsert/remove.
struct RecordingVector {
    inner: MemoryVector,
    rows: Mutex<BTreeMap<String, (String, Value)>>,
}

impl RecordingVector {
    fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            inner: MemoryVector::new(embedder, 0.2),
            rows: Mutex::new(BTreeMap::new()),
        }
    }

    /// Every stored row, id-ordered: `id\tcontent\tmeta-json`.
    fn rows(&self) -> Vec<String> {
        let rows = self.rows.lock().expect("vector lock");
        rows.iter()
            .map(|(id, (content, meta))| format!("{id}\t{content}\t{meta}"))
            .collect()
    }

    fn len(&self) -> usize {
        self.rows.lock().expect("vector lock").len()
    }

    fn rows_for(&self, ids: &[String]) -> Vec<String> {
        let rows = self.rows.lock().expect("vector lock");
        ids.iter()
            .filter_map(|id| {
                rows.get(id)
                    .map(|(content, meta)| format!("{id}\t{content}\t{meta}"))
            })
            .collect()
    }
}

#[async_trait]
impl VectorStore for RecordingVector {
    async fn upsert(&self, rows: Vec<VectorRow>) -> StoreResult<()> {
        {
            let mut mirror = self.rows.lock().expect("vector lock");
            for row in &rows {
                mirror.insert(row.id.clone(), (row.content.clone(), row.meta.clone()));
            }
        }
        self.inner.upsert(rows).await
    }

    async fn query(&self, query: &str, top_k: usize) -> StoreResult<Vec<VectorHit>> {
        self.inner.query(query, top_k).await
    }

    async fn remove(&self, ids: &[String]) -> StoreResult<()> {
        {
            let mut mirror = self.rows.lock().expect("vector lock");
            mirror.retain(|id, _| !ids.contains(id));
        }
        self.inner.remove(ids).await
    }

    async fn index_done(&self) -> StoreResult<()> {
        self.inner.index_done().await
    }
}

/// One pipeline over memory backends, recording vector stores included.
fn build() -> (
    Pipeline,
    Arc<RoutedLlm>,
    Arc<RecordingVector>,
    Arc<RecordingVector>,
    Arc<RecordingVector>,
) {
    let routed = Arc::new(RoutedLlm::new("mock-best", routing()));
    let llm = CachedLlm::new(routed.clone(), Arc::new(MemoryKv::new()));
    let repair = Arc::new(RepairQueue::new(Arc::new(MemoryKv::new())));
    let embedder = Arc::new(MockEmbedder::new());
    let entities_vdb = Arc::new(RecordingVector::new(embedder.clone()));
    let relationships_vdb = Arc::new(RecordingVector::new(embedder.clone()));
    let chunks_vdb = Arc::new(RecordingVector::new(embedder));

    let pipeline = Pipeline {
        full_docs: Arc::new(MemoryKv::new()),
        text_chunks: Arc::new(MemoryKv::new()),
        community_reports: Arc::new(MemoryKv::new()),
        graph: Arc::new(MemoryGraph::new()),
        entities_vdb: entities_vdb.clone() as Arc<dyn VectorStore>,
        relationships_vdb: Some(relationships_vdb.clone() as Arc<dyn VectorStore>),
        chunks_vdb: Some(chunks_vdb.clone() as Arc<dyn VectorStore>),
        tracking: TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        full_entities: Arc::new(MemoryKv::new()),
        full_relations: Arc::new(MemoryKv::new()),
        llm,
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        repair,
        options: PipelineOptions {
            enable_naive_rag: true,
            ..PipelineOptions::default()
        },
    };
    (
        pipeline,
        routed,
        entities_vdb,
        relationships_vdb,
        chunks_vdb,
    )
}

/// Canonical snapshot of every persisted row: KV truth tables row-for-row,
/// graph node/edge rows, every vector store's rows, and the call counts.
#[derive(Debug, PartialEq)]
struct Snapshot {
    kv: BTreeMap<String, Vec<String>>,
    graph: Vec<String>,
    entity_vectors: Vec<String>,
    relation_vectors: Vec<String>,
    chunk_vectors: Vec<String>,
    llm_calls: usize,
    repair_pending: usize,
}

async fn kv_rows(store: &dyn KvStore) -> StoreResult<Vec<String>> {
    let keys = store.all_keys().await?;
    let rows = store.get_by_ids(&keys).await?;
    let mut out = keys
        .into_iter()
        .zip(rows)
        .map(|(key, row)| format!("{key}\t{}", row.unwrap_or(Value::Null)))
        .collect::<Vec<_>>();
    out.sort();
    Ok(out)
}

async fn graph_rows(graph: &dyn GraphStore) -> StoreResult<Vec<String>> {
    let snapshot = graph.snapshot().await?;
    let mut out: Vec<String> = snapshot
        .nodes
        .iter()
        .map(|(name, row)| format!("node\t{name}\t{row}"))
        .chain(
            snapshot
                .edges
                .iter()
                .map(|(src, tgt, row)| format!("edge\t{src}\t{tgt}\t{row}")),
        )
        .collect();
    out.sort();
    Ok(out)
}

async fn full_snapshot(
    pipeline: &Pipeline,
    routed: &RoutedLlm,
    entities: &RecordingVector,
    relations: &RecordingVector,
    chunks: &RecordingVector,
) -> StoreResult<Snapshot> {
    let mut kv = BTreeMap::new();
    kv.insert(
        "full_docs".to_string(),
        kv_rows(pipeline.full_docs.as_ref()).await?,
    );
    kv.insert(
        "text_chunks".to_string(),
        kv_rows(pipeline.text_chunks.as_ref()).await?,
    );
    kv.insert(
        "entity_chunks".to_string(),
        kv_rows(pipeline.tracking.entity_chunks.as_ref()).await?,
    );
    kv.insert(
        "relation_chunks".to_string(),
        kv_rows(pipeline.tracking.relation_chunks.as_ref()).await?,
    );
    kv.insert(
        "full_entities".to_string(),
        kv_rows(pipeline.full_entities.as_ref()).await?,
    );
    kv.insert(
        "full_relations".to_string(),
        kv_rows(pipeline.full_relations.as_ref()).await?,
    );
    kv.insert(
        "community_reports".to_string(),
        kv_rows(pipeline.community_reports.as_ref()).await?,
    );
    Ok(Snapshot {
        kv,
        graph: graph_rows(pipeline.graph.as_ref()).await?,
        entity_vectors: entities.rows(),
        relation_vectors: relations.rows(),
        chunk_vectors: chunks.rows(),
        llm_calls: routed.calls(),
        repair_pending: pipeline.repair.pending().await?.len(),
    })
}

/// The rows owned by A/B's exclusive elements: the graph rows, the tracking
/// rows, and the vector ids. The shared SOLSTICE rows are deliberately not
/// in this set.
fn exclusive_row_prefixes() -> Vec<&'static str> {
    vec![
        "node\tVESPERINE\t",
        "node\tORLOVE\t",
        "edge\tSOLSTICE\tVESPERINE\t",
        "edge\tORLOVE\tSOLSTICE\t",
    ]
}

fn exclusive_tracking_prefixes() -> Vec<&'static str> {
    vec![
        "VESPERINE\t",
        "ORLOVE\t",
        "SOLSTICE<SEP>VESPERINE\t",
        "ORLOVE<SEP>SOLSTICE\t",
    ]
}

fn exclusive_vector_ids() -> (Vec<String>, Vec<String>) {
    let entities = vec![
        compute_mdhash_id("VESPERINE", "ent-"),
        compute_mdhash_id("ORLOVE", "ent-"),
    ];
    let relations = [("SOLSTICE", "VESPERINE"), ("ORLOVE", "SOLSTICE")]
        .into_iter()
        .flat_map(|(a, b)| {
            [
                compute_mdhash_id(&format!("{a}{b}"), "rel-"),
                compute_mdhash_id(&format!("{b}{a}"), "rel-"),
            ]
        })
        .collect();
    (entities, relations)
}

async fn chunk_ids_for_doc(pipeline: &Pipeline, doc: &str) -> Vec<String> {
    let doc_key = compute_mdhash_id(doc, "doc-");
    let keys = pipeline.text_chunks.all_keys().await.expect("chunk keys");
    let mut out = Vec::new();
    for key in keys {
        let row = pipeline
            .text_chunks
            .get_by_id(&key)
            .await
            .expect("chunk row");
        if row
            .and_then(|r| {
                r.get("full_doc_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .as_deref()
            == Some(doc_key.as_str())
        {
            out.push(key);
        }
    }
    out.sort();
    out
}

fn rows_with(rows: &[String], prefixes: &[&'static str]) -> Vec<String> {
    rows.iter()
        .filter(|row| prefixes.iter().any(|prefix| row.starts_with(prefix)))
        .map(String::clone)
        .collect()
}

#[tokio::test]
async fn reinserting_known_documents_is_a_byte_identical_no_op() {
    // TAC1: re-inserting the same documents adds no rows, changes no row,
    // and spends no LLM calls.
    let (pipeline, routed, entities, relations, chunks) = build();
    let first = pipeline
        .insert(vec![DOC_A.to_string(), DOC_B.to_string()])
        .await
        .expect("first insert");
    assert_eq!(
        first,
        InsertOutcome::Inserted {
            docs: 2,
            chunks: 2,
            entities: 3,
            relations: 2
        },
        "the two documents share SOLSTICE, so three nodes and two relations"
    );
    let before = full_snapshot(&pipeline, &routed, &entities, &relations, &chunks)
        .await
        .expect("snapshot");

    // The snapshot must be non-trivial: both KV truth tables carry the
    // committed rows, the graph carries every element, the vector stores
    // carry one row per entity and two (forward+reverse) per relation.
    assert_eq!(
        before.llm_calls, 4,
        "one extract + one gleaning round per chunk"
    );
    assert_eq!(before.kv["full_docs"].len(), 2);
    assert_eq!(before.kv["text_chunks"].len(), 2);
    assert_eq!(
        before.kv["entity_chunks"].len(),
        3,
        "one tracking row per entity"
    );
    assert_eq!(before.kv["relation_chunks"].len(), 2);
    assert_eq!(before.kv["full_entities"].len(), 2);
    assert_eq!(before.kv["full_relations"].len(), 2);
    assert!(
        before
            .graph
            .iter()
            .any(|row| row.starts_with("node\tSOLSTICE\t")),
        "shared entity row exists: {before:?}"
    );
    assert_eq!(entities.len(), 3);
    assert_eq!(relations.len(), 4, "two pairs, written in both directions");
    assert_eq!(chunks.len(), 2);
    assert_eq!(before.repair_pending, 0);
    assert!(routed.unmatched_prompts().is_empty());

    // Re-insert the identical documents: the filter_keys short-circuit must
    // leave the entire persisted state byte-identical and skip extraction.
    let reinserted = pipeline
        .insert(vec![DOC_A.to_string(), DOC_B.to_string()])
        .await
        .expect("re-insert");
    assert_eq!(
        reinserted,
        InsertOutcome::AllDocsKnown,
        "the docs are already committed"
    );
    let after = full_snapshot(&pipeline, &routed, &entities, &relations, &chunks)
        .await
        .expect("snapshot");
    assert_eq!(after, before, "re-insert must be a byte-identical no-op");
    assert_eq!(
        after.llm_calls, 4,
        "no LLM calls: the extract/keywords cache is untouched by a short-circuit"
    );
    assert!(routed.unmatched_prompts().is_empty());
}

#[tokio::test]
async fn inserting_an_unrelated_document_keeps_unrelated_rows_byte_identical() {
    // FAC1: the incremental path only touches the shared SOLSTICE rows and
    // C's own rows; every row owned by A/B's exclusive elements keeps its
    // stored bytes.
    let (pipeline, routed, entities, relations, chunks) = build();
    pipeline
        .insert(vec![DOC_A.to_string(), DOC_B.to_string()])
        .await
        .expect("first insert");
    let base = full_snapshot(&pipeline, &routed, &entities, &relations, &chunks)
        .await
        .expect("snapshot");
    let (exclusive_entities, exclusive_relations) = exclusive_vector_ids();
    let exclusive_chunk_ids = [
        chunk_ids_for_doc(&pipeline, DOC_A).await,
        chunk_ids_for_doc(&pipeline, DOC_B).await,
    ]
    .concat();
    // The exclusive rows as stored before C arrives: every one of them must
    // survive the third insert with its exact bytes.
    let base_entity_rows = entities.rows_for(&exclusive_entities);
    let base_relation_rows = relations.rows_for(&exclusive_relations);
    let base_chunk_rows = chunks.rows_for(&exclusive_chunk_ids);
    assert!(
        !base_entity_rows.is_empty()
            && !base_relation_rows.is_empty()
            && !base_chunk_rows.is_empty(),
        "the invariant set must be non-empty"
    );

    let third = pipeline
        .insert(vec![DOC_C.to_string()])
        .await
        .expect("third insert");
    assert_eq!(
        third,
        InsertOutcome::Inserted {
            docs: 1,
            chunks: 1,
            entities: 2,
            relations: 1
        },
        "C brings SOLSTICE (shared) plus its exclusive GAMMA FIELD"
    );
    let after = full_snapshot(&pipeline, &routed, &entities, &relations, &chunks)
        .await
        .expect("snapshot");

    // The exclusive graph rows — node and edge rows — keep their exact bytes.
    let prefixes = exclusive_row_prefixes();
    assert_eq!(
        rows_with(&base.graph, &prefixes),
        rows_with(&after.graph, &prefixes),
        "exclusive element graph rows must be untouched"
    );
    assert!(
        !rows_with(&base.graph, &prefixes).is_empty(),
        "the invariant set must be non-empty"
    );

    // The exclusive tracking rows keep their exact bytes.
    let tracking_prefixes = exclusive_tracking_prefixes();
    for namespace in ["entity_chunks", "relation_chunks"] {
        assert_eq!(
            rows_with(&base.kv[namespace], &tracking_prefixes),
            rows_with(&after.kv[namespace], &tracking_prefixes),
            "{namespace} exclusive rows must be untouched"
        );
    }

    // The exclusive vector rows keep their exact bytes (id + content + meta).
    assert_eq!(
        entities.rows_for(&exclusive_entities),
        base_entity_rows,
        "no A/B exclusive entity vector row may be rewritten"
    );
    assert_eq!(
        relations.rows_for(&exclusive_relations),
        base_relation_rows,
        "no A/B exclusive relation vector row may be rewritten"
    );
    assert_eq!(
        chunks.rows_for(&exclusive_chunk_ids),
        base_chunk_rows,
        "no A/B chunk vector row may be rewritten"
    );

    // The shared SOLSTICE rows are the only allowed graph changes: its node
    // gained C's description fragment, its edge rows are C's own.
    let solstice_rows = |snapshot: &Snapshot| rows_with(&snapshot.graph, &["node\tSOLSTICE\t"]);
    assert_ne!(
        solstice_rows(&base),
        solstice_rows(&after),
        "the shared entity row must have been merged with C's fragment"
    );
    assert!(
        solstice_rows(&after)[0].contains("SOLSTICE researchers fund trials"),
        "the merged SOLSTICE description carries C's fragment: {:?}",
        solstice_rows(&after)
    );
    let solstice_vector = compute_mdhash_id("SOLSTICE", "ent-");
    let solstice_prefix = format!("{solstice_vector}\t");
    assert_ne!(
        base.entity_vectors
            .iter()
            .find(|row| row.starts_with(&solstice_prefix))
            .cloned(),
        after
            .entity_vectors
            .iter()
            .find(|row| row.starts_with(&solstice_prefix))
            .cloned(),
        "the shared entity's vector row must have been re-embedded"
    );

    // The only new LLM work is C's own extract + gleaning round.
    assert_eq!(
        after.llm_calls - base.llm_calls,
        2,
        "one extract + one gleaning round for the new chunk"
    );
    assert_eq!(after.repair_pending, 0);
    assert!(
        routed.unmatched_prompts().is_empty(),
        "every prompt must match a rule"
    );
}
