// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `CacheTier` trait default implementations.

use cachelon_tier::{CacheEntry, CacheTier, Error};
use std::collections::HashMap;
use std::sync::Mutex;

/// Minimal implementation that only provides required methods
struct MinimalCache<K, V> {
    data: Mutex<HashMap<K, CacheEntry<V>>>,
}

impl<K, V> MinimalCache<K, V> {
    fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl<K, V> CacheTier<K, V> for MinimalCache<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.data.lock().expect("lock poisoned").get(key).cloned())
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.data.lock().expect("lock poisoned").insert(key.clone(), entry);
        Ok(())
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.data.lock().expect("lock poisoned").remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        self.data.lock().expect("lock poisoned").clear();
        Ok(())
    }
}

#[tokio::test]
async fn minimal_cachelon_get_miss() {
    let cache = MinimalCache::<String, i32>::new();
    let result: Option<CacheEntry<i32>> = cache.get(&"key".to_string()).await.expect("error on get");
    assert!(result.is_none());
}

#[tokio::test]
async fn minimal_cachelon_get_hit() {
    let cache = MinimalCache::<String, i32>::new();
    let _: () = cache
        .insert(&"key".to_string(), CacheEntry::new(42))
        .await
        .expect("error on insert");
    let result: Option<CacheEntry<i32>> = cache.get(&"key".to_string()).await.expect("error on get");
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 42);
}

#[tokio::test]
async fn default_insert_wraps_insert() {
    let cache = MinimalCache::<String, i32>::new();
    let _: () = cache
        .insert(&"key".to_string(), CacheEntry::new(42))
        .await
        .expect("error on insert");
    let result: Option<CacheEntry<i32>> = cache.get(&"key".to_string()).await.expect("error on get");
    assert!(result.is_some());
}

#[tokio::test]
async fn default_invalidate_returns_ok() {
    let cache = MinimalCache::<String, i32>::new();

    // Should return Ok even for nonexistent keys
    let _: () = cache.invalidate(&"nonexistent".to_string()).await.unwrap();

    // Should return Ok for existing keys
    let _: () = cache
        .insert(&"key".to_string(), CacheEntry::new(42))
        .await
        .expect("error on insert");
    let _: () = cache.invalidate(&"key".to_string()).await.unwrap();
}

#[tokio::test]
async fn default_clear_returns_ok() {
    let cache = MinimalCache::<String, i32>::new();

    // Should return Ok for empty cache
    let _: () = cache.clear().await.unwrap();

    // Should return Ok even with entries
    let _: () = cache
        .insert(&"key".to_string(), CacheEntry::new(42))
        .await
        .expect("error on insert");
    let _: () = cache.clear().await.unwrap();
}

#[tokio::test]
async fn default_len_returns_none() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    assert!(cache.len().is_none());
}

#[tokio::test]
async fn default_is_empty_returns_none() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    assert!(cache.is_empty().is_none());
}

/// Implementation that provides `len()` to test `is_empty()` default behavior
struct CacheWithLen<K, V> {
    data: Mutex<HashMap<K, CacheEntry<V>>>,
}

impl<K, V> CacheWithLen<K, V> {
    fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl<K, V> CacheTier<K, V> for CacheWithLen<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.data.lock().expect("lock poisoned").get(key).cloned())
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.data.lock().expect("lock poisoned").insert(key.clone(), entry);
        Ok(())
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.data.lock().expect("lock poisoned").remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        self.data.lock().expect("lock poisoned").clear();
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        Some(self.data.lock().expect("lock poisoned").len() as u64)
    }
}

#[tokio::test]
async fn is_empty_uses_len_when_available() {
    let cache = CacheWithLen::<String, i32>::new();

    // Empty cache
    assert_eq!(cache.is_empty(), Some(true));
    assert_eq!(cache.len(), Some(0));

    // Add entry
    let _: () = cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
    assert_eq!(cache.is_empty(), Some(false));
    assert_eq!(cache.len(), Some(1));
}
