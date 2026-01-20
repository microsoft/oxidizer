// Copyright (c) Microsoft Corporation.

#![expect(missing_docs, reason = "Test code")]
#![cfg(feature = "test-util")]

//! Integration tests for fallback cache behavior.
//!
//! Note: Tests for internal behavior (promotion policy internals, refresh mechanism)
//! are in the unit tests in `src/fallback.rs`.

use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
use cachelon_tier::testing::MockCache;
use tick::Clock;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn fallback_cachelon_miss_in_both() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let result = cache.get(&"nonexistent".to_string()).await;
        assert!(result.is_none());
    });
}

#[test]
fn fallback_cachelon_hit_in_primary() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;

        let result = cache.get(&key).await;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[test]
fn fallback_cachelon_insert_goes_to_both() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;

        assert!(cache.get(&key).await.is_some());
    });
}

#[test]
fn fallback_cachelon_invalidate_clears_both() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        cache.invalidate(&key).await;

        assert!(cache.get(&key).await.is_none());
    });
}

#[test]
fn fallback_cachelon_clear() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await;
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await;

        cache.clear().await;

        assert!(cache.get(&"k1".to_string()).await.is_none());
        assert!(cache.get(&"k2".to_string()).await.is_none());
    });
}

#[test]
fn fallback_cachelon_try_operations() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let key = "key".to_string();

        assert!(cache.try_insert(&key, CacheEntry::new(42)).await.is_ok());
        let result = cache.try_get(&key).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
        assert!(cache.try_invalidate(&key).await.is_ok());
        assert!(cache.try_clear().await.is_ok());
    });
}

#[test]
fn fallback_cachelon_len_returns_some() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().fallback(fallback).build();

        let len = cache.len();
        assert!(len.is_some());

        let is_empty = cache.is_empty();
        assert!(is_empty.is_some());
    });
}

fn failing_cache() -> MockCache<String, i32> {
    let cache = MockCache::new();
    cache.fail_when(|_| true);
    cache
}

#[test]
fn fallback_cachelon_try_insert_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.try_insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_cachelon_try_invalidate_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.try_invalidate(&"key".to_string()).await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_cachelon_try_clear_error_propagation() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
        let fallback_storage = failing_cache();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.try_clear().await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_cachelon_try_get_error_from_primary() {
    block_on(async {
        let clock = Clock::new_frozen();

        let primary_storage = failing_cache();
        let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .fallback(fallback)
            .build();

        let result = cache.try_get(&"key".to_string()).await;
        assert!(result.is_err());
    });
}

#[test]
fn fallback_builder_with_promotion_policy_always() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::Always)
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());
    });
}

#[test]
fn fallback_builder_with_promotion_policy_never() {
    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::Never)
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());
    });
}

#[test]
fn fallback_builder_with_promotion_policy_when() {
    fn is_positive(entry: &CacheEntry<i32>) -> bool {
        *entry.value() > 0
    }

    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::when(is_positive))
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());
    });
}

#[test]
fn fallback_builder_with_promotion_policy_when_boxed() {
    let threshold = 10;

    block_on(async {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .promotion_policy(FallbackPromotionPolicy::when_boxed(move |entry: &CacheEntry<i32>| {
                *entry.value() >= threshold
            }))
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());
    });
}

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
            .promotion_policy(FallbackPromotionPolicy::Always);

        // L1 with nested fallback
        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(l2)
            .promotion_policy(FallbackPromotionPolicy::Never)
            .build();

        assert!(!cache.name().is_empty());

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());
    });
}
