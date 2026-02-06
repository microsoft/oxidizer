// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `InMemoryCache`.

use cachelon_memory::{InMemoryCache, InMemoryCacheBuilder};
use cachelon_tier::{CacheEntry, CacheTier};
use std::time::Duration;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn new_creates_unbounded_cache() {
    let cache = InMemoryCache::<String, i32>::new();
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn with_capacity_creates_bounded_cache() {
    let cache = InMemoryCache::<String, i32>::with_capacity(100);
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn default_creates_unbounded_cache() {
    let cache = InMemoryCache::<String, i32>::default();
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn get_returns_none_for_missing_key() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        let result = cache.get(&"missing".to_string()).await.expect("get failed");
        assert!(result.is_none());
    });
}

#[test]
fn insert_and_get_returns_value() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        let entry = cache
            .get(&"key".to_string())
            .await
            .expect("get failed")
            .expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

#[test]
fn insert_overwrites_existing_value() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
        cache.insert(&"key".to_string(), CacheEntry::new(100)).await.expect("insert failed");

        let entry = cache
            .get(&"key".to_string())
            .await
            .expect("get failed")
            .expect("entry should exist");
        assert_eq!(*entry.value(), 100);
    });
}

#[test]
fn get_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();

        let result = cache.get(&"missing".to_string()).await;
        assert!(result.is_ok());
        assert!(result.expect("get failed").is_none());

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
        let result = cache.get(&"key".to_string()).await;
        assert!(result.is_ok());
        assert!(result.expect("get failed").is_some());
    });
}

#[test]
fn insert_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
        assert!(cache.get(&"key".to_string()).await.expect("get failed").is_some());
    });
}

#[test]
fn invalidate_removes_entry() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        cache.invalidate(&"key".to_string()).await.expect("invalidate failed");

        let result = cache.get(&"key".to_string()).await.expect("get failed");
        assert!(result.is_none());
    });
}

#[test]
fn invalidate_nonexistent_key_succeeds() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.invalidate(&"nonexistent".to_string()).await.expect("invalidate failed");
    });
}

#[test]
fn invalidate_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        cache.invalidate(&"key".to_string()).await.expect("invalidate failed");
        assert!(cache.get(&"key".to_string()).await.expect("get failed").is_none());
    });
}

#[test]
fn clear_removes_all_entries() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key1".to_string(), CacheEntry::new(1)).await.expect("insert failed");
        cache.insert(&"key2".to_string(), CacheEntry::new(2)).await.expect("insert failed");
        cache.insert(&"key3".to_string(), CacheEntry::new(3)).await.expect("insert failed");

        cache.clear().await.expect("clear failed");

        assert!(cache.get(&"key1".to_string()).await.expect("get failed").is_none());
        assert!(cache.get(&"key2".to_string()).await.expect("get failed").is_none());
        assert!(cache.get(&"key3".to_string()).await.expect("get failed").is_none());
    });
}

#[test]
fn clear_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        cache.clear().await.expect("clear failed");
    });
}

#[test]
fn len_returns_some() {
    // Note: moka uses eventual consistency for entry counts,
    // so we only verify that len() returns Some(_)
    let cache = InMemoryCache::<String, i32>::new();
    assert!(cache.len().is_some());
}


#[test]
fn clone_shares_underlying_cache() {
    block_on(async {
        let cache1 = InMemoryCache::<String, i32>::new();
        let cache2 = cache1.clone();

        cache1.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        let entry = cache2
            .get(&"key".to_string())
            .await
            .expect("get failed")
            .expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

// Builder tests

#[test]
fn builder_default_creates_unbounded_cache() {
    let cache = InMemoryCacheBuilder::<String, i32>::default().build();
    assert_eq!(cache.len(), Some(0));
}

#[test]
fn builder_max_capacity_sets_limit() {
    let _cache = InMemoryCacheBuilder::<String, i32>::new().max_capacity(100).build();
}

#[test]
fn builder_initial_capacity_preallocates() {
    let _cache = InMemoryCacheBuilder::<String, i32>::new().initial_capacity(50).build();
}

#[test]
fn builder_time_to_live_sets_ttl() {
    let _cache = InMemoryCacheBuilder::<String, i32>::new()
        .time_to_live(Duration::from_secs(300))
        .build();
}

#[test]
fn builder_time_to_idle_sets_tti() {
    let _cache = InMemoryCacheBuilder::<String, i32>::new()
        .time_to_idle(Duration::from_secs(60))
        .build();
}

#[test]
fn builder_name_sets_cache_name() {
    let _cache = InMemoryCacheBuilder::<String, i32>::new().name("test-cache").build();
}

#[test]
fn builder_all_options_combined() {
    let cache = InMemoryCacheBuilder::<String, i32>::new()
        .max_capacity(1000)
        .initial_capacity(100)
        .time_to_live(Duration::from_secs(300))
        .time_to_idle(Duration::from_secs(60))
        .name("full-config-cache")
        .build();

    assert_eq!(cache.len(), Some(0));
}
