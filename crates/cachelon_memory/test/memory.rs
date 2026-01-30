// Copyright (c) Microsoft Corporation.

#![cfg(feature = "test-util")]

//! Integration tests for InMemoryCache.

use cachelon::{CacheEntry, CacheTier, InMemoryCache};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn new_creates_empty_cache() {
    let cache: InMemoryCache<String, i32> = InMemoryCache::new();
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn default_creates_empty_cache() {
    let cache: InMemoryCache<String, i32> = Default::default();
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn with_capacity_creates_empty_cache() {
    let cache: InMemoryCache<String, i32> = InMemoryCache::with_capacity(100);
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn get_insert_operations() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();

        let key = "key".to_string();

        assert!(cache.get(&key).await.is_none());

        cache.insert(&key, CacheEntry::new(42)).await;

        let entry = cache.get(&key).await;
        assert!(entry.is_some());
        assert_eq!(*entry.unwrap().value(), 42);
    });
}

#[test]
fn try_get_try_insert() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();
        let key = "key".to_string();

        assert!(cache.try_get(&key).await.is_ok());
        assert!(cache.try_insert(&key, CacheEntry::new(100)).await.is_ok());
        assert!(cache.try_get(&key).await.unwrap().is_some());
    });
}

#[test]
fn invalidate_removes_entry() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();
        let key = "key".to_string();

        cache.insert(&key, CacheEntry::new(42)).await;
        assert!(cache.get(&key).await.is_some());

        cache.invalidate(&key).await;
        assert!(cache.get(&key).await.is_none());
    });
}

#[test]
fn try_invalidate_returns_ok() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();
        let key = "key".to_string();

        assert!(cache.try_invalidate(&key).await.is_ok());
    });
}

#[test]
fn clear_removes_all_entries() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await;
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await;

        cache.clear().await;

        assert!(cache.get(&"k1".to_string()).await.is_none());
        assert!(cache.get(&"k2".to_string()).await.is_none());
    });
}

#[test]
fn try_clear_returns_ok() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;

        let result = cache.try_clear().await;
        assert!(result.is_ok());
    });
}

#[test]
fn len_returns_entry_count() {
    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::new();

        assert_eq!(cache.len(), Some(0));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await;

        assert!(cache.len().is_some());
    });
}

#[test]
fn builder_with_multiple_options() {
    use std::time::Duration;

    block_on(async {
        let cache: InMemoryCache<String, i32> = InMemoryCache::builder()
            .max_capacity(1000)
            .initial_capacity(100)
            .time_to_live(Duration::from_secs(300))
            .time_to_idle(Duration::from_secs(60))
            .name("test-cache")
            .build();

        // Verify cache is empty initially
        assert_eq!(cache.len(), Some(0));

        // Insert and retrieve
        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await;

        let entry = cache.get(&key).await;
        assert!(entry.is_some());
        assert_eq!(*entry.unwrap().value(), 42);

        // Verify length after insert
        assert_eq!(cache.len(), Some(1));
    });
}
