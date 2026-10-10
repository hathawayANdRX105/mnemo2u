//! Offline end-to-end: insert → graph/tracking → three query modes.
//! In-memory stores + content-routed mock LLM (no network, no model download).

use std::sync::Arc;

use mnemo2u::core::rag::{QueryMode, QueryParam};
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::{MockEmbedder, RoutedLlm};
use mnemo2u::pipeline::{CommunityMode, InsertOutcome, Pipeline, PipelineOptions};
use mnemo2u::store::memory::{MemoryGraph, MemoryKv, MemoryVector};
use mnemo2u::store::repair::RepairQueue;
use mnemo2u::Tokenizer;

const EXTRACTION: &str = "entity<|#|>ACME<|#|>organization<|#|>Acme makes things.\n\
entity<|#|>BETA LABS<|#|>organization<|#|>Beta Labs makes robots.\n\
relation<|#|>ACME<|#|>BETA LABS<|#|>ACME acquired Beta Labs.<|#|>partnership\n\
<|COMPLETE|>";
const GLEAN: &str = "entity<|#|>GAMMA<|#|>organization<|#|>Gamma ships software.";
const REPORT: &str = r#"{"title": "Community", "summary": "ACME and Beta Labs are linked.", "findings": [{"summary": "Acquisition", "explanation": "ACME acquired Beta Labs."}], "rating": 7.5}"#;
const KEYWORDS: &str =
    r#"{"high_level_keywords": ["partnership"], "low_level_keywords": ["ACME"]}"#;
const ANSWER: &str = "ACME acquired Beta Labs in 2024.";

/// Routing keys are matched against the prompt text, so each key names a phrase
/// that only the reference's own prompt for that stage contains.
fn routing() -> Vec<(String, String)> {
    vec![
        (
            "Extract entities and relationships".to_string(),
            EXTRACTION.to_string(),
        ),
        (
            "missed or incorrectly formatted".to_string(),
            GLEAN.to_string(),
        ),
        (
            "general information discovery".to_string(),
            REPORT.to_string(),
        ),
        ("high_level_keywords".to_string(), KEYWORDS.to_string()),
        ("Multiple Paragraphs".to_string(), ANSWER.to_string()),
    ]
}

fn build_pipeline(enable_naive_rag: bool, community: CommunityMode) -> (Pipeline, Arc<RoutedLlm>) {
    let routed = Arc::new(RoutedLlm::new("mock-best", routing()));
    let cache = Arc::new(MemoryKv::new());
    let llm = CachedLlm::new(routed.clone(), cache);
    let repair = Arc::new(RepairQueue::new(Arc::new(MemoryKv::new())));
    let embedder = Arc::new(MockEmbedder::new());

    let pipeline = Pipeline {
        full_docs: Arc::new(MemoryKv::new()),
        text_chunks: Arc::new(MemoryKv::new()),
        community_reports: Arc::new(MemoryKv::new()),
        graph: Arc::new(MemoryGraph::new()),
        entities_vdb: Arc::new(MemoryVector::new(embedder.clone(), 0.2)),
        relationships_vdb: Some(Arc::new(MemoryVector::new(embedder.clone(), 0.2))
            as Arc<dyn mnemo2u::core::traits::VectorStore>),
        tracking: mnemo2u::index::tracking::TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        full_entities: Arc::new(MemoryKv::new()),
        full_relations: Arc::new(MemoryKv::new()),
        chunks_vdb: enable_naive_rag.then(|| {
            Arc::new(MemoryVector::new(embedder, 0.2))
                as Arc<dyn mnemo2u::core::traits::VectorStore>
        }),
        llm,
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        repair,
        options: PipelineOptions {
            community_mode: community,
            ..PipelineOptions::default()
        },
    };
    (pipeline, routed)
}

#[tokio::test]
async fn insert_builds_graph_and_tracking_without_community_calls() {
    // Default policy: the insert path never detects communities and never
    // generates reports (stage doc §0 decision 3).
    let (pipeline, routed) = build_pipeline(true, CommunityMode::Off);
    let outcome = pipeline
        .insert(vec![
            "ACME acquired Beta Labs in 2024.".to_string(),
            "Beta Labs builds robots for ACME.".to_string(),
        ])
        .await
        .expect("insert");

    match outcome {
        InsertOutcome::Inserted {
            docs,
            chunks,
            entities,
            relations,
        } => {
            assert_eq!((docs, chunks), (2, 2));
            assert_eq!(
                (entities, relations),
                (3, 1),
                "two chunk entities plus the gleaning entity, one relation"
            );
        }
        other => panic!("unexpected outcome {other:?}"),
    }

    // Graph rows keep the reference's normalised identifiers and lowercase
    // entity types; the relation weight is the evidence floor (two chunks).
    let acme = pipeline
        .graph
        .get_node("ACME")
        .await
        .expect("graph read")
        .expect("ACME node");
    assert_eq!(acme["entity_type"], "organization");
    assert!(acme["description"]
        .as_str()
        .expect("description")
        .contains("Acme makes things"));
    let edge = pipeline
        .graph
        .get_edge("ACME", "BETA LABS")
        .await
        .expect("graph read")
        .expect("relation");
    assert_eq!(edge["weight"], 2.0, "one relation per chunk: 1 + 1");
    assert!(edge["source_id"]
        .as_str()
        .expect("source")
        .contains("chunk-"));

    // Tracking rows exist for both entities and the merged relation.
    let entity_keys = pipeline
        .tracking
        .entity_chunks
        .all_keys()
        .await
        .expect("entity tracking");
    assert_eq!(entity_keys.len(), 3, "one row per entity, GAMMA included");
    let relation_keys = pipeline
        .tracking
        .relation_chunks
        .all_keys()
        .await
        .expect("relation tracking");
    assert_eq!(
        relation_keys.len(),
        1,
        "the sorted pair is one tracking row: {relation_keys:?}"
    );

    // No community work happened on the default policy.
    assert!(pipeline
        .community_reports
        .all_keys()
        .await
        .expect("reports")
        .is_empty());
    assert_eq!(
        routed.calls(),
        4,
        "extraction plus one gleaning round per chunk, no report"
    );
    assert!(
        routed.unmatched_prompts().is_empty(),
        "every prompt must match a rule"
    );

    // Documents and chunks are committed.
    assert_eq!(pipeline.full_docs.all_keys().await.expect("docs").len(), 2);
    assert_eq!(
        pipeline.text_chunks.all_keys().await.expect("chunks").len(),
        2
    );

    // Re-inserting the same documents is a no-op (filter_keys path).
    let outcome = pipeline
        .insert(vec![
            "ACME acquired Beta Labs in 2024.".to_string(),
            "Beta Labs builds robots for ACME.".to_string(),
        ])
        .await
        .expect("re-insert");
    assert_eq!(outcome, InsertOutcome::AllDocsKnown);
}

#[tokio::test]
async fn community_mode_on_demand_rebuilds_reports_on_insert() {
    // `on_demand` rebuilds reports from the current graph during insert, which
    // is the only mode that spends a report call.
    let (pipeline, routed) = build_pipeline(true, CommunityMode::OnDemand);
    pipeline
        .insert(vec!["ACME acquired Beta Labs in 2024.".to_string()])
        .await
        .expect("insert");

    let reports = pipeline
        .community_reports
        .all_keys()
        .await
        .expect("reports");
    assert!(!reports.is_empty(), "on_demand must produce reports");
    assert!(
        routed.unmatched_prompts().is_empty(),
        "every prompt must match a rule, community report included"
    );
}

#[tokio::test]
async fn query_modes_return_context_and_answers() {
    let (pipeline, _) = build_pipeline(true, CommunityMode::Off);
    pipeline
        .insert(vec!["ACME acquired Beta Labs in 2024.".to_string()])
        .await
        .expect("insert");

    // local: entity vector hit → context tables.
    let context = pipeline
        .query(
            "ACME",
            &QueryParam {
                mode: QueryMode::Local,
                only_need_context: true,
                ..QueryParam::default()
            },
        )
        .await
        .expect("local query");
    assert!(
        context.contains("Knowledge Graph Data (Entity):"),
        "local context: {context}"
    );
    assert!(context.contains("ACME"));

    // local full answer goes through the llm.
    let answer = pipeline
        .query(
            "ACME",
            &QueryParam {
                mode: QueryMode::Local,
                ..QueryParam::default()
            },
        )
        .await
        .expect("local answer");
    assert_eq!(answer, ANSWER);

    // global: the relation vector arm — the reference has no community map
    // phase in the query path at all.
    let global = pipeline
        .query(
            "what did ACME do?",
            &QueryParam {
                mode: QueryMode::Global,
                only_need_context: true,
                ..QueryParam::default()
            },
        )
        .await
        .expect("global query");
    assert!(
        global.contains("-----Relationships-----"),
        "global context: {global}"
    );
    assert!(global.contains("ACME"), "global context: {global}");

    // naive: chunk vector search.
    let naive = pipeline
        .query(
            "ACME robots",
            &QueryParam {
                mode: QueryMode::Naive,
                only_need_context: true,
                ..QueryParam::default()
            },
        )
        .await
        .expect("naive query");
    assert!(naive.contains("ACME"), "naive context: {naive}");
}

#[tokio::test]
async fn naive_mode_is_rejected_when_disabled() {
    let (pipeline, _) = build_pipeline(false, CommunityMode::Off);
    pipeline
        .insert(vec!["ACME acquired Beta Labs in 2024.".to_string()])
        .await
        .expect("insert");

    let result = pipeline
        .query(
            "ACME",
            &QueryParam {
                mode: QueryMode::Naive,
                ..QueryParam::default()
            },
        )
        .await;
    assert!(result.is_err(), "naive RAG must fail loudly when disabled");
}

#[tokio::test]
async fn local_mode_is_rejected_and_skips_entity_embeddings_when_disabled() {
    // `enable_local: false` (`graphrag.py:58`): the reference neither builds
    // `entities_vdb` nor allows local queries.
    let embedder: Arc<dyn mnemo2u::core::traits::Embedder> = Arc::new(MockEmbedder::new());

    let routed = Arc::new(RoutedLlm::new("mock-best", routing()));
    let pipeline = Pipeline {
        full_docs: Arc::new(MemoryKv::new()),
        text_chunks: Arc::new(MemoryKv::new()),
        community_reports: Arc::new(MemoryKv::new()),
        graph: Arc::new(MemoryGraph::new()),
        entities_vdb: Arc::new(MemoryVector::new(embedder.clone(), 0.2)),
        relationships_vdb: Some(Arc::new(MemoryVector::new(embedder, 0.2))
            as Arc<dyn mnemo2u::core::traits::VectorStore>),
        tracking: mnemo2u::index::tracking::TrackingStores {
            entity_chunks: Arc::new(MemoryKv::new()),
            relation_chunks: Arc::new(MemoryKv::new()),
        },
        full_entities: Arc::new(MemoryKv::new()),
        full_relations: Arc::new(MemoryKv::new()),
        chunks_vdb: None,
        llm: CachedLlm::new(routed, Arc::new(MemoryKv::new())),
        tokenizer: Tokenizer::for_gpt_4o().expect("tokenizer"),
        options: PipelineOptions {
            enable_local: false,
            ..PipelineOptions::default()
        },
        repair: Arc::new(RepairQueue::new(Arc::new(MemoryKv::new()))),
    };

    let outcome = pipeline
        .insert(vec!["ACME acquired Beta Labs in 2024.".to_string()])
        .await
        .expect("insert");
    assert!(matches!(
        outcome,
        InsertOutcome::Inserted { entities: 3, .. }
    ));

    // The store exists but was never written: no entity was embedded.
    let hits = pipeline.entities_vdb.query("ACME", 5).await.expect("query");
    assert!(
        hits.is_empty(),
        "enable_local=false must not embed entities"
    );

    // Local queries fail like the reference's guard instead of returning
    // something fabricated.
    let result = pipeline
        .query(
            "ACME",
            &QueryParam {
                mode: QueryMode::Local,
                ..QueryParam::default()
            },
        )
        .await;
    assert!(result.is_err(), "local mode must fail loudly when disabled");
}

#[tokio::test]
async fn embedding_batch_size_one_matches_the_default_batch() {
    // `embedding_batch_num` splits the embed calls; the result must not depend
    // on the split (same mock embedder, same rows).
    async fn run_with_batch(batch: usize) -> String {
        let (mut pipeline, _) = build_pipeline(false, CommunityMode::Off);
        pipeline.options.embedding_batch_num = batch;
        pipeline
            .insert(vec![
                "ACME acquired Beta Labs in 2024.".to_string(),
                "Beta Labs builds robots for ACME.".to_string(),
            ])
            .await
            .expect("insert");
        pipeline
            .query(
                "ACME",
                &QueryParam {
                    mode: QueryMode::Local,
                    only_need_context: true,
                    ..QueryParam::default()
                },
            )
            .await
            .expect("local query")
    }

    let default = run_with_batch(32).await;
    let single = run_with_batch(1).await;
    assert_eq!(default, single, "batching must not change retrieval");
    assert!(default.contains("ACME"), "local context: {default}");
}
