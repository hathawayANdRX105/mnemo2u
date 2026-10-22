//! LanceDB vector contract: merge-on-id upsert, cosine threshold, ranking,
//! persistence across reopen (a derived index must survive a restart).

use std::sync::Arc;

use mnemo2u::core::traits::{Embedder, VectorRow, VectorStore};
use mnemo2u::llm::mock::MockEmbedder;
use mnemo2u::store::lancedb::LanceVector;
use serde_json::json;

#[tokio::test]
async fn upsert_query_threshold_and_persistence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = dir.path().to_str().expect("utf8 path");
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new());

    {
        let store = LanceVector::open(uri, "text_chunks", embedder.clone(), 0.2)
            .await
            .expect("open");

        // A fresh table answers empty, not an error.
        assert!(store.query("cats", 5).await.expect("query").is_empty());
        assert!(store.query("cats", 0).await.expect("query").is_empty());
        assert!(
            store.upsert(vec![]).await.is_ok(),
            "empty upsert is a no-op"
        );

        store
            .upsert(vec![
                VectorRow {
                    id: "c1".into(),
                    content: "cats purr on the couch".into(),
                    meta: json!({"doc": "d1"}),
                },
                VectorRow {
                    id: "c2".into(),
                    content: "kubernetes schedules pods".into(),
                    meta: json!({"doc": "d2"}),
                },
            ])
            .await
            .expect("upsert");

        let hits = store.query("cats purr", 5).await.expect("query");
        assert_eq!(
            hits.len(),
            1,
            "the unrelated description must fall below the threshold"
        );
        assert_eq!(hits[0].id, "c1");
        // Mock embedder: bag-of-words cosine, two of five words shared.
        assert!(
            hits[0].distance > 0.6,
            "cosine similarity = {}",
            hits[0].distance
        );
        assert_eq!(hits[0].meta, json!({"doc": "d1"}), "meta must ride along");

        store.index_done().await.expect("index done");

        // Re-inserting an id rewrites the row instead of appending a second one.
        store
            .upsert(vec![VectorRow {
                id: "c1".into(),
                content: "kubernetes schedules pods".into(),
                meta: json!({"doc": "d1"}),
            }])
            .await
            .expect("update in place");
    }

    let reopened = LanceVector::open(uri, "text_chunks", embedder, 0.2)
        .await
        .expect("reopen");
    let hits = reopened.query("kubernetes pods", 5).await.expect("query");
    let mut ids: Vec<&str> = hits.iter().map(|hit| hit.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["c1", "c2"], "one row per id across reopen");
    assert!(
        reopened
            .query("cats purr", 5)
            .await
            .expect("query")
            .is_empty(),
        "the updated row must not answer for its old content"
    );
}
