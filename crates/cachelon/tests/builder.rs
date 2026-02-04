// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for Cache builder API.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, Error, FallbackPromotionPolicy};
use cachelon_tier::testing::{CacheOp, MockCache};
use tick::Clock;

type TestResult = Result<(), Error>;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn cache_builder_with_storage() -> TestResult {
    let clock = Clock::new_frozen();
    let storage = MockCache::<String, i32>::new();
    let cache = Cache::builder::<String, i32>(clock).storage(storage).build();

    block_on(async {
        assert!(cache.get(&"key".to_string()).await?.is_none());
        Ok(())
    })
}

#[test]
fn mock_cache_with_storage() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Cache operations work
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
        let value = cache.get(&"key".to_string()).await?;
        assert_eq!(*value.unwrap().value(), 42);

        // Mock handle records operations
        assert_eq!(mock.operations().len(), 2);
        Ok(())
    })
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
        result.unwrap_err();

        // Clear failures and get succeeds
        mock.clear_failures();
        let result = cache.get(&"key".to_string()).await;
        result.unwrap();
    });
}

#[test]
fn mock_cache_shares_state_with_handle() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        let cache = Cache::builder(clock).storage(mock.clone()).build();

        // Insert via cache
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        // Mock handle sees the data
        assert!(mock.contains_key(&"key".to_string()));
        assert_eq!(mock.entry_count(), 1);
        Ok(())
    })
}

#[test]
fn cache_builder_clock() {
    let clock = Clock::new_frozen();
    let builder = Cache::builder::<String, i32>(clock);
    let builder_clock = builder.clock();
    // Verify clock is accessible and returns valid instant
    let _ = builder_clock.instant();
}

#[test]
fn fallback_builder_basic() -> TestResult {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory().ttl(Duration::from_secs(3600));

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(fallback)
        .build();

    block_on(async {
        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_promotion_policy() -> TestResult {
    let clock = Clock::new_frozen();

    let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(fallback)
        .promotion_policy(FallbackPromotionPolicy::Never)
        .build();

    block_on(async {
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
        let entry = cache.get(&"key".to_string()).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_nested_fallback() -> TestResult {
    let clock = Clock::new_frozen();

    // L3 (deepest)
    let l3 = Cache::builder::<String, i32>(clock.clone()).memory();

    // L2 with its own fallback
    let l2 = Cache::builder::<String, i32>(clock.clone())
        .memory()
        .fallback(l3)
        .promotion_policy(FallbackPromotionPolicy::Always);

    // L1 with nested fallback
    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::Never)
        .build();

    block_on(async {
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
        let entry = cache.get(&"key".to_string()).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}
