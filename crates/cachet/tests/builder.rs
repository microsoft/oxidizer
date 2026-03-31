// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for Cache builder API.

#![cfg(feature = "memory")]

use std::time::Duration;

use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
use cachet_tier::{CacheOp, MockCache};
use tick::Clock;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn cache_builder_with_storage() {
    let clock = Clock::new_frozen();
    let storage = MockCache::<String, i32>::new();
    let cache = Cache::builder::<String, i32>(clock).storage(storage).build();

    block_on(async {
        assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
    });
}

#[tokio::test]
async fn mock_cache_with_storage() {
    let clock = Clock::new_frozen();
    let mock = MockCache::<String, i32>::new();
    let cache = Cache::builder(clock).storage(mock.clone()).build();

    // Cache operations work
    cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
    let value = cache.get(&"key".to_string()).await.unwrap();
    assert_eq!(*value.unwrap().value(), 42);

    // Mock handle records operations (insert + get)
    assert_eq!(mock.operations().len(), 2);
}

#[test]
fn mock_cache_failure_injection() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Configure failures
        mock.fail_when(|op| matches!(op, CacheOp::Get(_)));

        // get fails
        let result = cache.get(&"key".to_string()).await;
        result.expect_err("mock configured to fail on get");

        // Clear failures and get succeeds
        mock.clear_failures();
        let result = cache.get(&"key".to_string()).await;
        result.expect("get should succeed after clearing failures");
    });
}

#[test]
fn mock_cache_shares_state_with_handle() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Insert via cache
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();

        // Mock handle sees the data
        assert!(mock.contains_key(&"key".to_string()));
        assert_eq!(mock.entry_count(), 1);
    });
}

#[test]
fn cache_builder_clock() {
    let clock = Clock::new_frozen();
    let expected_instant = clock.instant();
    let builder = Cache::builder::<String, i32>(clock);

    // Verify builder exposes the same clock
    let builder_clock = builder.clock();
    assert_eq!(builder_clock.instant(), expected_instant);
}

#[cfg_attr(miri, ignore)]
#[test]
fn cache_builder_name() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().name("test_cache").build();
    assert_eq!(cache.name(), "test_cache");
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_basic() {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory().ttl(Duration::from_secs(3600));

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(fallback)
        .build();

    block_on(async {
        let key = "key".to_string();
        cache.insert(key.clone(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_promotion_policy() {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(fallback)
        .promotion_policy(FallbackPromotionPolicy::never())
        .build();

    block_on(async {
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&"key".to_string()).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_nested_fallback() {
    let clock = Clock::new_frozen();

    // L3 (deepest)
    let l3 = Cache::builder::<String, i32>(clock.clone()).memory();

    // L2 with its own fallback
    let l2 = Cache::builder::<String, i32>(clock.clone())
        .memory()
        .fallback(l3)
        .promotion_policy(FallbackPromotionPolicy::always());

    // L1 with nested fallback
    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::never())
        .build();

    block_on(async {
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&"key".to_string()).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}
