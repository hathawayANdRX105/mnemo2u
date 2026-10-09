//! Cached LLM client: second identical call must hit the cache, not the model
//! (`openai_complete_if_cache`, `_llm.py:44-61`).

use std::sync::Arc;

use mnemo2u::core::traits::KvStore;
use mnemo2u::llm::cache::CachedLlm;
use mnemo2u::llm::mock::MockLlm;
use mnemo2u::llm::ModelOptions;
use mnemo2u::store::memory::MemoryKv;

#[tokio::test]
async fn identical_calls_hit_the_cache() {
    let inner = Arc::new(MockLlm::new("gpt-4o", vec!["answer one".to_string()]));
    let cache = Arc::new(MemoryKv::new());
    let client = CachedLlm::new(inner.clone(), cache.clone());

    let (first, hit) = client
        .complete_cached("hi", None, &[], &ModelOptions::default())
        .await
        .expect("first call");
    assert_eq!(first, "answer one");
    assert!(!hit, "first call must be a miss");

    let (second, hit) = client
        .complete_cached("hi", None, &[], &ModelOptions::default())
        .await
        .expect("second call");
    assert_eq!(second, "answer one");
    assert!(hit, "identical call must be a cache hit");
    assert_eq!(inner.calls(), 1, "the model must be called exactly once");

    // The value shape mirrors the reference (`{"return", "model"}`).
    let keys = cache.all_keys().await.expect("cache keys");
    assert_eq!(keys.len(), 1);
    let row = cache
        .get_by_id(&keys[0])
        .await
        .expect("cache row")
        .expect("present");
    assert_eq!(row["return"], "answer one");
    assert_eq!(row["model"], "gpt-4o");
}

#[tokio::test]
async fn different_prompts_miss_and_burn_script() {
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec!["a".to_string(), "b".to_string()],
    ));
    let client = CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new()));

    let (first, _) = client
        .complete_cached("one", None, &[], &ModelOptions::default())
        .await
        .expect("call one");
    let (second, hit) = client
        .complete_cached("two", None, &[], &ModelOptions::default())
        .await
        .expect("call two");
    assert_eq!((first.as_str(), second.as_str()), ("a", "b"));
    assert!(!hit);
    assert_eq!(inner.calls(), 2);

    // System prompt participates in the key (reference builds messages first).
    let result = client
        .complete_cached("one", Some("sys"), &[], &ModelOptions::default())
        .await;
    assert!(
        result.is_err(),
        "exhausted script proves the system prompt changed the key"
    );
}
