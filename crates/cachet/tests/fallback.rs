// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for fallback cache behavior.
//!
//! Note: Tests for internal behavior (promotion policy internals, refresh mechanism)
//! are in the unit tests in `src/fallback.rs`.

#![cfg(feature = "memory")]

use std::time::Duration;

use anyspawn::Spawner;
use cachet::{Cache, CacheEntry, CacheTier, FallbackPromotionPolicy, TimeToRefresh};
use cachet_tier::MockCache;
use tick::Clock;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_miss_in_both() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let result = cache.get(&"nonexistent".to_string()).await.unwrap();
        assert!(result.is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_hit_in_primary() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();

        let result = cache.get(&key).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_insert_goes_to_both() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();

        assert!(cache.get(&key).await.unwrap().is_some());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_invalidate_clears_both() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        cache.invalidate(&key).await.unwrap();

        assert!(cache.get(&key).await.unwrap().is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_clear() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await.unwrap();
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await.unwrap();

        cache.clear().await.unwrap();

        assert!(cache.get(&"k1".to_string()).await.unwrap().is_none());
        assert!(cache.get(&"k2".to_string()).await.unwrap().is_none());
    });
}

#[test]
fn fallback_cache_len_returns_correct_count() {
    block_on(async {
        // Use MockCache for immediate consistency of len()
        let clock = Clock::new_frozen();

        let fallback = Cache::builder(clock.clone()).storage(MockCache::<String, i32>::new());

        let cache = Cache::builder(clock)
            .storage(MockCache::<String, i32>::new())
            .fallback(fallback)
            .build();

        assert_eq!(cache.len(), Some(0));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

        assert_eq!(cache.len(), Some(1));
    });
}

fn failing_cache() -> MockCache<String, i32> {
    let cache = MockCache::new();
    cache.fail_when(|_| true);
    cache
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_insert_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachet_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        result.unwrap_err();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_invalidate_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachet_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.invalidate(&"key".to_string()).await;
        result.unwrap_err();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_clear_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachet_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.clear().await;
        result.unwrap_err();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_cache_get_falls_back_on_primary_error() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        // When primary fails, fallback is checked (returns None since key doesn't exist there)
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_with_promotion_policy_always() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::always())
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_with_promotion_policy_never() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::never())
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_with_promotion_policy_when_boxed() {
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
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn nested_fallback_builder() {
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
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap();
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_get_triggers_promotion() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachet_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();

        fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        // get should trigger promotion from fallback
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_builder_stampede_protection() {
    block_on(async {
        let clock = Clock::new_frozen();
        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .stampede_protection()
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[cfg(feature = "logs")]
#[test]
fn fallback_builder_use_logs_emits_logs() {
    block_on(async {
        let capture = testing_aids::LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let clock = Clock::new_frozen();
        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().use_logs().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        cache.get(&key).await.unwrap().expect("entry should exist");

        // Verify logs were actually emitted
        capture.assert_contains("cache.inserted");
    });
}

#[cfg_attr(miri, ignore)]
#[cfg(feature = "logs")]
#[test]
fn cache_builder_use_logs_emits_logs() {
    block_on(async {
        let capture = testing_aids::LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().use_logs().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        cache.get(&key).await.unwrap().expect("entry should exist");

        // Verify logs were actually emitted (catches with_logs mutation to false)
        capture.assert_contains("cache.inserted");
        capture.assert_contains("cache.hit");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn cache_builder_clock_returns_clock() {
    let clock = Clock::new_frozen();
    let builder = Cache::builder::<String, i32>(clock).memory();
    // clock() method on CacheBuilder
    let _ = builder.clock();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn fallback_builder_time_to_refresh_does_not_panic() {
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
    cache.insert(&key, CacheEntry::new(42)).await.unwrap();
    let entry = cache.get(&key).await.unwrap().expect("entry should exist");
    assert_eq!(*entry.value(), 42);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn do_refresh_updates_primary_from_fallback() {
    // Verifies do_refresh actually fetches from fallback and promotes to primary
    let control = tick::ClockControl::new();
    let clock = control.to_clock();
    let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();
    fallback_storage.insert(&"key".to_string(), CacheEntry::new(99)).await.unwrap();

    let primary_storage = cachet_memory::InMemoryCache::<String, i32>::new();
    let primary_check = primary_storage.clone();

    // Insert a stale entry directly into primary with cached_at set so TTR check triggers.
    // Must set cached_at because CacheWrapper checks value.cached_at() for refresh eligibility.
    let mut stale_entry = CacheEntry::new(42);
    stale_entry.ensure_cached_at(clock.system_time());
    primary_storage.insert(&"key".to_string(), stale_entry).await.unwrap();

    let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);
    let ttr = TimeToRefresh::new(Duration::from_nanos(1), Spawner::new_tokio());

    let cache = Cache::builder::<String, i32>(clock)
        .storage(primary_storage)
        .fallback(fallback)
        .time_to_refresh(ttr)
        .build();

    let key = "key".to_string();

    // Advance the clock so the ttr duration elapses
    control.advance(Duration::from_millis(5));

    // get triggers background refresh (primary has stale 42, fallback has fresh 99)
    let result = cache.get(&key).await.unwrap();
    assert!(result.is_some());

    // Wait for background refresh to complete
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Primary should now have the refreshed value from fallback
    let refreshed = primary_check.get(&key).await.unwrap();
    assert!(refreshed.is_some());
    assert_eq!(*refreshed.unwrap().value(), 99);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn do_refresh_deduplicates_in_flight() {
    // Exercises do_refresh deduplication: second call with same key is a no-op
    let control = tick::ClockControl::new();
    let clock = control.to_clock();
    let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();
    fallback_storage.insert(&"key".to_string(), CacheEntry::new(99)).await.unwrap();

    let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);
    let ttr = TimeToRefresh::new(Duration::from_nanos(1), Spawner::new_tokio());

    let cache = Cache::builder::<String, i32>(clock)
        .memory()
        .fallback(fallback)
        .time_to_refresh(ttr)
        .build();

    // Insert a stale entry
    let key = "key".to_string();
    cache.insert(&key, CacheEntry::new(42)).await.unwrap();

    // Advance the clock so the ttr duration elapses
    control.advance(Duration::from_millis(5));

    // get triggers background refresh
    let result = cache.get(&key).await.unwrap();
    assert!(result.is_some());

    // Second get also triggers do_refresh; duplicate is detected and skipped
    let result2 = cache.get(&key).await.unwrap();
    assert!(result2.is_some());
}

#[cfg_attr(miri, ignore)]
#[cfg(feature = "metrics")]
#[test]
fn fallback_builder_use_metrics() {
    block_on(async {
        let tester = testing_aids::MetricTester::new();
        let clock = Clock::new_frozen();
        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .use_metrics(tester.meter_provider())
            .fallback(fallback)
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&key).await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_get_error_from_fallback_tier() {
    block_on(async {
        let clock = Clock::new_frozen();

        // Primary miss + fallback error → error propagates
        let primary_storage = cachet_memory::InMemoryCache::<String, i32>::new();
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

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_get_promotion_failure_still_returns_value() {
    block_on(async {
        let clock = Clock::new_frozen();

        // Primary fails on insert (promotion), fallback has the value
        let primary_storage = MockCache::<String, i32>::new();
        primary_storage.fail_when(|op| matches!(op, cachet_tier::CacheOp::Insert { .. }));

        let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();
        fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        // get should return the value despite promotion failure
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_insert_primary_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(result.is_err(), "primary insert error should propagate");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_invalidate_primary_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.invalidate(&"key".to_string()).await;
        assert!(result.is_err(), "primary invalidate error should propagate");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn fallback_clear_primary_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachet_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.clear().await;
        assert!(result.is_err(), "primary clear error should propagate");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn nested_fallback_three_tier_chain() {
    block_on(async {
        let clock = Clock::new_frozen();

        // Build a 3-tier cache: L1 (primary) -> L2 (fallback) -> L3 (fallback-of-fallback)
        // This exercises FallbackBuilder::fallback() (line 425) by calling .fallback()
        // on the FallbackBuilder returned by the first .fallback() call.
        let l3 = Cache::builder::<String, i32>(clock.clone()).memory();
        let l1_with_l2 = Cache::builder::<String, i32>(clock.clone())
            .memory()
            .fallback(Cache::builder::<String, i32>(clock).memory());
        // This calls FallbackBuilder.fallback() — NOT CacheBuilder.fallback()
        let cache = l1_with_l2.fallback(l3).build();

        // Insert and retrieve through the 3-tier hierarchy
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = cache.get(&"key".to_string()).await.unwrap().expect("entry should exist");
        assert_eq!(*entry.value(), 42);

        // Clear and verify
        cache.clear().await.unwrap();
        assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
    });
}
