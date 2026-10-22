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

/// `enable_llm_cache_for_extract=False` (`lightrag.py:1096`): the extract
/// namespace bypasses the cache in both directions — the same input burns the
/// model twice and no row is written.
#[tokio::test]
async fn extract_switch_off_calls_the_model_every_time() {
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec!["first".to_string(), "second".to_string()],
    ));
    let cache = Arc::new(MemoryKv::new());
    let client =
        CachedLlm::new(inner.clone(), cache.clone()).with_enable_llm_cache_for_extract(false);
    let options = ModelOptions::default();

    let (first, first_hit) = client
        .complete_cached_keyed("same input", None, &[], &options, "extract")
        .await
        .expect("call one");
    let (second, second_hit) = client
        .complete_cached_keyed("same input", None, &[], &options, "extract")
        .await
        .expect("call two");
    assert_eq!((first.as_str(), second.as_str()), ("first", "second"));
    assert!(
        !first_hit && !second_hit,
        "with the switch off neither call may be served from the cache"
    );
    assert_eq!(
        inner.calls(),
        2,
        "off: the identical second call must hit the model"
    );
    assert!(
        cache.all_keys().await.expect("keys").is_empty(),
        "off: no cache row may be written"
    );
}

/// With the default switches, an extract call repeats for free: the second
/// identical input is a hit and the model is called exactly once.
#[tokio::test]
async fn extract_switch_on_is_default_and_hits_the_cache() {
    let inner = Arc::new(MockLlm::new("gpt-4o", vec!["one".to_string()]));
    let cache = Arc::new(MemoryKv::new());
    let client = CachedLlm::new(inner.clone(), cache.clone());
    let options = ModelOptions::default();

    let (_, first_hit) = client
        .complete_cached_keyed("same input", None, &[], &options, "extract")
        .await
        .expect("call one");
    let (_, second_hit) = client
        .complete_cached_keyed("same input", None, &[], &options, "extract")
        .await
        .expect("call two");
    assert!(!first_hit, "the first call is the miss");
    assert!(second_hit, "the repeat must be a cache hit");
    assert_eq!(inner.calls(), 1, "on: the repeat burns zero model calls");
    assert_eq!(cache.all_keys().await.expect("keys").len(), 1);
}

/// `enable_llm_cache=False` (`lightrag.py:1093`) turns the global switch: the
/// keyword namespace bypasses the cache in both directions.
#[tokio::test]
async fn global_switch_off_bypasses_the_keyword_cache() {
    let inner = Arc::new(MockLlm::new(
        "gpt-4o",
        vec!["a".to_string(), "b".to_string()],
    ));
    let cache = Arc::new(MemoryKv::new());
    let client = CachedLlm::new(inner.clone(), cache.clone()).with_enable_llm_cache(false);
    let options = ModelOptions::default();

    let (_, first_hit) = client
        .complete_cached_keyed("query", None, &[], &options, "keywords")
        .await
        .expect("call one");
    let (_, second_hit) = client
        .complete_cached_keyed("query", None, &[], &options, "keywords")
        .await
        .expect("call two");
    assert!(
        !first_hit && !second_hit,
        "the keyword namespace must not read or write"
    );
    assert_eq!(inner.calls(), 2);
    assert!(cache.all_keys().await.expect("keys").is_empty());
}

/// The two switches are independent: with only the global one off, the extract
/// namespace keeps caching (the reference's `handle_cache` split,
/// `utils.py:4464-4468`).
#[tokio::test]
async fn global_switch_off_leaves_extract_cached() {
    let inner = Arc::new(MockLlm::new("gpt-4o", vec!["one".to_string()]));
    let client =
        CachedLlm::new(inner.clone(), Arc::new(MemoryKv::new())).with_enable_llm_cache(false);
    let options = ModelOptions::default();

    let (_, _) = client
        .complete_cached_keyed("input", None, &[], &options, "extract")
        .await
        .expect("call one");
    let (_, hit) = client
        .complete_cached_keyed("input", None, &[], &options, "extract")
        .await
        .expect("call two");
    assert!(
        hit,
        "the extract namespace follows its own switch, still on"
    );
    assert_eq!(inner.calls(), 1);
}
