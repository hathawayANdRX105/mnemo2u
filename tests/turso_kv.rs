//! Turso KV contract: roundtrip, filter semantics, namespaces, persistence.
//!
//! One handle at a time: the file is opened, used, dropped, then reopened —
//! the same lifecycle the pipeline uses (open on start, close on exit).

use mnemo2u::core::traits::KvStore;
use mnemo2u::store::turso::TursoKv;
use serde_json::json;

#[tokio::test]
async fn kv_roundtrip_filter_and_persistence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("kv.db");
    let path = path.to_str().expect("path");

    {
        let kv = TursoKv::open(path, "full_docs").await.expect("open");
        kv.upsert(vec![
            ("a".to_string(), json!({"content": "hello"})),
            ("b".to_string(), json!({"content": "world"})),
        ])
        .await
        .expect("upsert");

        assert_eq!(kv.all_keys().await.expect("keys").len(), 2);
        let row = kv.get_by_id("a").await.expect("get").expect("row");
        assert_eq!(row["content"], "hello");
        assert!(kv.get_by_id("missing").await.expect("get").is_none());

        // filter_keys returns the keys that are NOT present.
        let unknown = kv
            .filter_keys(&["a".to_string(), "zzz".to_string()])
            .await
            .expect("filter");
        assert_eq!(unknown, vec!["zzz".to_string()]);

        let rows = kv
            .get_by_ids(&["a".to_string(), "missing".to_string()])
            .await
            .expect("batch");
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_some());
        assert!(rows[1].is_none());

        // Upsert replaces.
        kv.upsert(vec![("a".to_string(), json!({"content": "updated"}))])
            .await
            .expect("update");
        assert_eq!(
            kv.get_by_id("a").await.expect("get").expect("row")["content"],
            "updated"
        );
    }

    // Re-opening sees the same data (the truth layer survives restarts).
    {
        let reopened = TursoKv::open(path, "full_docs").await.expect("reopen");
        assert_eq!(reopened.all_keys().await.expect("keys").len(), 2);

        // A different namespace on the same database is empty.
        drop(reopened);
        let other = TursoKv::open(path, "text_chunks")
            .await
            .expect("open other");
        assert!(other.all_keys().await.expect("keys").is_empty());
    }

    // drop_all clears only its namespace.
    {
        let kv = TursoKv::open(path, "full_docs").await.expect("reopen");
        kv.drop_all().await.expect("drop");
        assert!(kv.all_keys().await.expect("keys").is_empty());
        drop(kv);
        let other = TursoKv::open(path, "text_chunks")
            .await
            .expect("open other");
        assert!(other.all_keys().await.expect("keys").is_empty());
    }
}
