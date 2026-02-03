// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for fallback cache behavior.
//!
//! Note: Tests for internal behavior (promotion policy internals, refresh mechanism)
//! are in the unit tests in `src/fallback.rs`.

use cachelon::{Cache, CacheEntry, Error, FallbackPromotionPolicy};
use cachelon_tier::testing::MockCache;
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
fn fallback_cache_len_returns_some() {
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
            .promotion_policy(FallbackPromotionPolicy::Always)
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
            .promotion_policy(FallbackPromotionPolicy::Never)
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn fallback_builder_with_promotion_policy_when() -> TestResult {
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
            .promotion_policy(FallbackPromotionPolicy::when_boxed(move |entry: &CacheEntry<i32>| {
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
            .promotion_policy(FallbackPromotionPolicy::Always);

        // L1 with nested fallback
        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(l2)
            .promotion_policy(FallbackPromotionPolicy::Never)
            .build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        let entry = cache.get(&key).await?;
        assert_eq!(*entry.unwrap().value(), 42);
        Ok(())
    })
}
