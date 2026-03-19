// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `InMemoryCache`.

use std::time::Duration;

use cachet_memory::{InMemoryCache, InMemoryCacheBuilder};
use cachet_tier::{CacheEntry, CacheTier};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[cfg_attr(miri, ignore)]
#[test]
fn new_creates_unbounded_cache() {
    let cache = InMemoryCache::<String, i32>::new();
    assert_eq!(cache.len(), Some(0));
}

#[cfg_attr(miri, ignore)]
#[test]
fn with_capacity_creates_bounded_cache() {
    let cache = InMemoryCache::<String, i32>::with_capacity(100);
    assert_eq!(cache.len(), Some(0));
}

#[cfg_attr(miri, ignore)]
#[test]
fn default_creates_unbounded_cache() {
    let cache = InMemoryCache::<String, i32>::default();
    assert_eq!(cache.len(), Some(0));
}

#[cfg_attr(miri, ignore)]
#[test]
fn get_returns_none_for_missing_key() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        let result = cache.get(&"missing".to_string()).await.expect("get failed");
        assert!(result.is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn insert_and_get_returns_value() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        let entry = cache
            .get(&"key".to_string())
            .await
            .expect("get failed")
            .expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn insert_overwrites_existing_value() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
        cache.insert("key".to_string(), CacheEntry::new(100)).await.expect("insert failed");

        let entry = cache
            .get(&"key".to_string())
            .await
            .expect("get failed")
            .expect("entry should exist");
        assert_eq!(*entry.value(), 100);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn get_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();

        let result = cache.get(&"missing".to_string()).await;
        assert!(result.is_ok());
        assert!(result.expect("get failed").is_none());

        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
        let result = cache.get(&"key".to_string()).await;
        assert!(result.is_ok());
        assert!(result.expect("get failed").is_some());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn insert_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
        assert!(cache.get(&"key".to_string()).await.expect("get failed").is_some());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn invalidate_removes_entry() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        cache.invalidate(&"key".to_string()).await.expect("invalidate failed");

        let result = cache.get(&"key".to_string()).await.expect("get failed");
        assert!(result.is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn invalidate_nonexistent_key_succeeds() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.invalidate(&"nonexistent".to_string()).await.expect("invalidate failed");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn invalidate_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        cache.invalidate(&"key".to_string()).await.expect("invalidate failed");
        assert!(cache.get(&"key".to_string()).await.expect("get failed").is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn clear_removes_all_entries() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key1".to_string(), CacheEntry::new(1)).await.expect("insert failed");
        cache.insert("key2".to_string(), CacheEntry::new(2)).await.expect("insert failed");
        cache.insert("key3".to_string(), CacheEntry::new(3)).await.expect("insert failed");

        cache.clear().await.expect("clear failed");

        assert!(cache.get(&"key1".to_string()).await.expect("get failed").is_none());
        assert!(cache.get(&"key2".to_string()).await.expect("get failed").is_none());
        assert!(cache.get(&"key3".to_string()).await.expect("get failed").is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn clear_returns_ok() {
    block_on(async {
        let cache = InMemoryCache::<String, i32>::new();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        cache.clear().await.expect("clear failed");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn len_returns_some_zero_for_empty_cache() {
    let cache = InMemoryCache::<String, i32>::new();
    assert_eq!(cache.len(), Some(0));
}

#[cfg_attr(miri, ignore)]
#[test]
fn len_returns_some_not_none() {
    // Moka's entry_count() is eventually consistent, so we can't assert exact
    // counts immediately after insert. But we can verify that len() returns
    // Some (not None), which catches the mutation `len -> None`.
    let cache = InMemoryCache::<String, i32>::new();
    assert!(cache.len().is_some());
}

#[cfg_attr(miri, ignore)]
#[test]
fn clone_shares_underlying_cache() {
    block_on(async {
        let cache1 = InMemoryCache::<String, i32>::new();
        let cache2 = cache1.clone();

        cache1.insert("key".to_string(), CacheEntry::new(42)).await.expect("insert failed");

        let entry = cache2
            .get(&"key".to_string())
            .await
            .expect("get failed")
            .expect("entry should exist");
        assert_eq!(*entry.value(), 42);
    });
}

// Builder tests

#[cfg_attr(miri, ignore)]
#[test]
fn builder_default_creates_unbounded_cache() {
    let cache = InMemoryCacheBuilder::<String, i32>::default().build().expect("build failed");
    assert_eq!(cache.len(), Some(0));
}

#[cfg_attr(miri, ignore)]
#[test]
fn builder_all_options_combined() {
    let cache = InMemoryCacheBuilder::<String, i32>::new()
        .max_capacity(1000)
        .initial_capacity(100)
        .time_to_live(Duration::from_secs(300))
        .time_to_idle(Duration::from_secs(60))
        .name("full-config-cache")
        .build()
        .expect("build failed");

    assert_eq!(cache.len(), Some(0));
}
