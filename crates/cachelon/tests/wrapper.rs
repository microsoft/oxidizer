// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `CacheWrapper` public API (through Cache).

use cachelon::{Cache, CacheEntry, Error};
use std::time::Duration;
use tick::Clock;

type TestResult = Result<(), Error>;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn wrapper_name() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();
    let wrapper = cache.inner();
    assert!(!wrapper.name().is_empty());
}

#[test]
fn wrapper_get_miss() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let result = cache.get(&"nonexistent".to_string()).await?;
        assert!(result.is_none());
        Ok(())
    })
}

#[test]
fn wrapper_get_hit() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;

        let result = cache.get(&key).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn wrapper_insert() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;

        assert!(cache.get(&key).await?.is_some());
        Ok(())
    })
}

#[test]
fn wrapper_invalidate() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;
        cache.invalidate(&key).await?;

        assert!(cache.get(&key).await?.is_none());
        Ok(())
    })
}

#[test]
fn wrapper_clear() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await?;
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await?;

        cache.clear().await?;

        assert!(cache.get(&"k1".to_string()).await?.is_none());
        assert!(cache.get(&"k2".to_string()).await?.is_none());
        Ok(())
    })
}

#[test]
fn wrapper_len_and_is_empty() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        assert_eq!(cache.len(), Some(0));
        assert_eq!(cache.is_empty(), Some(true));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;

        // After insert, len() and is_empty() return Some values
        // Note: exact count may be eventually consistent with moka cache
        assert!(cache.len().is_some());
        assert!(cache.is_empty().is_some());
        Ok(())
    })
}

#[test]
fn wrapper_with_ttl_configured() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().ttl(Duration::from_secs(60)).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;

        // Entry should exist immediately after insertion
        let result = cache.get(&key).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn wrapper_entry_with_ttl() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        // Entry with per-entry TTL
        let entry = CacheEntry::with_ttl(42, Duration::from_secs(120));
        cache.insert(&key, entry).await?;

        // Entry should exist immediately after insertion
        let result = cache.get(&key).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}

#[test]
fn wrapper_no_ttl_configured() -> TestResult {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await?;

        // Entry should exist (no TTL configured)
        let result = cache.get(&key).await?;
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
        Ok(())
    })
}
