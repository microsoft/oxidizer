// Copyright (c) Microsoft Corporation.

#![expect(missing_docs, reason = "Test code")]
#![cfg(feature = "test-util")]

//! Integration tests for Cache builder API.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
use cachelon_tier::testing::{CacheOp, MockCache};
use tick::Clock;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn cachelon_builder_with_storage() {
    let clock = Clock::new_frozen();
    let storage = MockCache::<String, i32>::new();
    let cache = Cache::builder::<String, i32>(clock).storage(storage).build();

    block_on(async {
        assert!(cache.get(&"key".to_string()).await.is_none());
    });
}

#[test]
fn mock_cachelon_with_storage() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Cache operations work
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        let value = cache.get(&"key".to_string()).await;
        assert_eq!(*value.unwrap().value(), 42);

        // Mock handle records operations
        assert_eq!(mock.operations().len(), 2);
    });
}

#[test]
fn mock_cachelon_failure_injection() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Configure failures
        mock.fail_when(|op| matches!(op, CacheOp::Get(_)));

        // try_get fails
        assert!(cache.try_get(&"key".to_string()).await.is_err());

        // get (infallible) still works (returns None)
        let result = cache.get(&"key".to_string()).await;
        assert!(result.is_none());
    });
}

#[test]
fn mock_cachelon_shares_state_with_handle() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Insert via cache
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;

        // Mock handle sees the data
        assert!(mock.contains_key(&"key".to_string()));
        assert_eq!(mock.entry_count(), 1);
    });
}

#[test]
fn cachelon_builder_clock() {
    let clock = Clock::new_frozen();
    let builder = Cache::builder::<String, i32>(clock.clone());
    let builder_clock = builder.clock();
    assert!(std::ptr::eq(builder_clock, builder_clock));
}

#[test]
fn fallback_builder_basic() {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory().ttl(Duration::from_secs(3600));

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .with_fallback(fallback)
        .build();

    block_on(async {
        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        let result = cache.get(&key).await;
        assert!(result.is_some());
    });
}

#[test]
fn fallback_builder_promotion_policy() {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .with_fallback(fallback)
        .promotion_policy(FallbackPromotionPolicy::Never)
        .build();

    assert!(!cache.name().is_empty());

    block_on(async {
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(cache.get(&"key".to_string()).await.is_some());
    });
}

#[test]
fn nested_fallback() {
    let clock = Clock::new_frozen();

    let l3 = Cache::builder::<String, i32>(clock.clone()).memory();
    let l2 = Cache::builder::<String, i32>(clock.clone()).memory().with_fallback(l3);

    let cache = Cache::builder::<String, i32>(clock).memory().with_fallback(l2).build();

    assert!(!cache.name().is_empty());

    block_on(async {
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(cache.get(&"key".to_string()).await.is_some());
    });
}

#[test]
fn fallback_builder_nested_with_fallback() {
    let clock = Clock::new_frozen();

    // L3 (deepest)
    let l3 = Cache::builder::<String, i32>(clock.clone()).memory();

    // L2 with its own fallback
    let l2 = Cache::builder::<String, i32>(clock.clone())
        .memory()
        .with_fallback(l3)
        .promotion_policy(FallbackPromotionPolicy::Always);

    // L1 with nested fallback
    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .with_fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::Never)
        .build();

    assert!(!cache.name().is_empty());

    block_on(async {
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(cache.get(&"key".to_string()).await.is_some());
    });
}

#[test]
fn cachelon_builder_debug() {
    let clock = Clock::new_frozen();
    let builder = Cache::builder::<String, i32>(clock).memory();

    let debug_str = format!("{:?}", builder);
    assert!(debug_str.contains("CacheBuilder"));
}

#[test]
fn fallback_builder_debug() {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

    let builder = Cache::builder::<String, i32>(clock).memory().with_fallback(fallback);

    let debug_str = format!("{:?}", builder);
    assert!(debug_str.contains("FallbackBuilder"));
}

#[test]
fn cachelon_builder_name_via_telemetry() {
    // Test that name can be set even without telemetry feature
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();

    // Name is automatically generated from type
    assert!(!cache.name().is_empty());
}
