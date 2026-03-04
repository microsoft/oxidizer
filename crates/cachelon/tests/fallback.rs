// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for fallback cache behavior.
//!
//! Note: Tests for internal behavior (promotion policy internals, refresh mechanism)
//! are in the unit tests in `src/fallback.rs`.

#![cfg(feature = "memory")]

use anyspawn::Spawner;
use cachelon::refresh::TimeToRefresh;
use cachelon::{Cache, CacheEntry, CacheTier, Error, FallbackPromotionPolicy};
use cachelon_tier::testing::MockCache;
use std::time::Duration;
use tick::Clock;

type TestResult = Result<(), Error>;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn fallback_cache_miss_in_both() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let result = cache.get(&"nonexistent".to_string()).await?;
        assert!(result.is_none());
        Ok(())
    })
}

#[test]
fn fallback_cache_hit_in_primary() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;

        let result = cache.get(&key).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_cache_insert_goes_to_both() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;

        assert!(cache.get(&key).await?.is_some());
        Ok(())
    })
}

#[test]
fn fallback_cache_invalidate_clears_both() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        cache.invalidate(&key).await?;

        assert!(cache.get(&key).await?.is_none());
        Ok(())
    })
}

#[test]
fn fallback_cache_clear() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await?;
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await?;

        cache.clear().await?;

        assert!(cache.get(&"k1".to_string()).await?.is_none());
        assert!(cache.get(&"k2".to_string()).await?.is_none());
        Ok(())
    })
}

#[test]
fn fallback_cache_len_returns_some() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        // Empty cache should have len 0
        assert_eq!(cache.len(), Some(0));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        // After insert, len returns Some (exact value may be eventually consistent with moka)
        assert!(cache.len().is_some());

        // Verify the entry is actually accessible
        let entry = cache.get(&"key".to_string()).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

fn failing_cache() -> MockCache<String, i32> {
    let cache = MockCache::new();
    cache.fail_when(|_| true);
    cache
}

#[test]
fn fallback_cache_insert_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_cache_invalidate_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.invalidate(&"key".to_string()).await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_cache_clear_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.clear().await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_cache_get_falls_back_on_primary_error() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        // When primary fails, fallback is checked (returns None since key doesn't exist there)
        let result = cache.get(&"key".to_string()).await?;
        assert!(result.is_none());
        Ok(())
    })
}

#[test]
fn fallback_builder_with_promotion_policy_always() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::always())
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_with_promotion_policy_never() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::never())
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_with_promotion_policy_when_boxed() -> TestResult {
    let threshold = 10;

    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::when(move |entry: &CacheEntry<i32>| {
                *entry.value() >= threshold
            }))
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn nested_fallback_builder() -> TestResult {
    block_on(async {
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

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_get_triggers_promotion() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

        fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        // get should trigger promotion from fallback
        let result = cache.get(&"key".to_string()).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_stampede_protection() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .stampede_protection()
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_use_logs() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        // This exercises the use_logs path on FallbackBuilder
        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).use_logs().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn cache_builder_use_logs() -> TestResult {
    block_on(async {
        // Exercises CacheBuilder::use_logs path
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().use_logs().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn cache_builder_clock_returns_clock() {
    let clock = Clock::new_frozen();
    let builder = Cache::builder::<String, i32>(clock.clone()).memory();
    // clock() method on CacheBuilder
    let _ = builder.clock();
}

#[tokio::test]
async fn fallback_builder_time_to_refresh_does_not_panic() -> TestResult {
    // Exercises time_to_refresh on FallbackBuilder. The background refresh
    // task is fire-and-forget, we just verify the cache is usable.
    let clock = Clock::new_frozen();
    let fallback = Cache::builder::<String, i32>(clock.clone()).memory();
    let ttr = TimeToRefresh::new(Duration::from_nanos(1), Spawner::new_tokio());

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(fallback)
        .time_to_refresh(ttr)
        .build();

    let key = "key".to_string();
    cache.insert(&key, CacheEntry::new(42)).await?;
    let entry = cache.get(&key).await?.expect("entry should exist");
    assert_eq!(*entry.value(), 42);
    Ok(())
}

#[tokio::test]
async fn do_refresh_deduplicates_in_flight() -> TestResult {
    // Exercises do_refresh deduplication: second call with same key is a no-op
    use cachelon::refresh::TimeToRefresh;

    let clock = Clock::new_frozen();
    let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
    fallback_storage.insert(&"key".to_string(), CacheEntry::new(99)).await?;

    let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);
    let ttr = TimeToRefresh::new(Duration::from_nanos(1), Spawner::new_tokio());

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(fallback)
        .time_to_refresh(ttr)
        .build();

    // Insert a stale entry
    let key = "key".to_string();
    cache.insert(&key, CacheEntry::new(42)).await?;

    // Sleep so the ttr duration elapses
    std::thread::sleep(Duration::from_millis(5));

    // get triggers background refresh
    let result = cache.get(&key).await?;
    assert!(result.is_some());

    // Second get also triggers do_refresh; duplicate is detected and skipped
    let result2 = cache.get(&key).await?;
    assert!(result2.is_some());

    Ok(())
}

#[cfg(feature = "metrics")]
#[test]
fn fallback_builder_use_metrics() -> TestResult {
    block_on(async {
        let tester = testing_aids::MetricTester::new();
        let clock = Clock::new_frozen();
        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .use_metrics(tester.meter_provider())
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?.expect("entry should exist");
        assert_eq!(*entry.value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_get_error_from_fallback_tier() {
    block_on(async {
        let clock = Clock::new_frozen();

        // Primary miss + fallback error → error propagates
        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.get(&"key".to_string()).await;
        assert!(result.is_err(), "fallback error should propagate on primary miss");
    });
}

#[test]
fn fallback_get_promotion_failure_still_returns_value() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();

        // Primary fails on insert (promotion), fallback has the value
        let primary_storage = MockCache::<String, i32>::new();
        primary_storage.fail_when(|op| matches!(op, cachelon_tier::testing::CacheOp::Insert { .. }));

        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        // get should return the value despite promotion failure
        let result = cache.get(&"key".to_string()).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_insert_primary_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(result.is_err(), "primary insert error should propagate");
    });
}

#[test]
fn fallback_invalidate_primary_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.invalidate(&"key".to_string()).await;
        assert!(result.is_err(), "primary invalidate error should propagate");
    });
}

#[test]
fn fallback_clear_primary_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.clear().await;
        assert!(result.is_err(), "primary clear error should propagate");
    });
}
