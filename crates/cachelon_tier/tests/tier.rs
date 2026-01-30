// Copyright (c) Microsoft Corporation.

//! Integration tests for `CacheTier` trait default implementations.

use cachelon_tier::{CacheEntry, CacheTier};
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
    async fn get(&self, key: &K) -> Option<CacheEntry<V>> {
        self.data.lock().expect("lock poisoned").get(key).cloned()
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) {
        self.data.lock().expect("lock poisoned").insert(key.clone(), entry);
    }
}

#[tokio::test]
async fn minimal_cachelon_get_miss() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    let result = cache.get(&"key".to_string()).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn minimal_cachelon_get_hit() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    let result = cache.get(&"key".to_string()).await;
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 42);
}

#[tokio::test]
async fn default_try_get_wraps_get() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    let result = cache.try_get(&"key".to_string()).await.unwrap();
    assert!(result.is_none());

    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    let result = cache.try_get(&"key".to_string()).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn default_try_insert_wraps_insert() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    cache.try_insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert!(cache.get(&"key".to_string()).await.is_some());
}

#[tokio::test]
async fn default_invalidate_does_not_panic() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();

    // Test passes if this doesn't panic (default impl is no-op)
    cache.invalidate(&"nonexistent".to_string()).await;

    // Test with existing key
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    cache.invalidate(&"key".to_string()).await;
}

#[tokio::test]
async fn default_try_invalidate_returns_ok() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();

    // Should return Ok even for nonexistent keys
    cache.try_invalidate(&"nonexistent".to_string()).await.unwrap();

    // Should return Ok for existing keys
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    cache.try_invalidate(&"key".to_string()).await.unwrap();
}

#[tokio::test]
async fn default_clear_does_not_panic() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();

    // Test passes if this doesn't panic (default impl is no-op)
    cache.clear().await;

    // Test with entries
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    cache.clear().await;
}

#[tokio::test]
async fn default_try_clear_returns_ok() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();

    // Should return Ok for empty cache
    cache.try_clear().await.unwrap();

    // Should return Ok even with entries
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    cache.try_clear().await.unwrap();
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
    async fn get(&self, key: &K) -> Option<CacheEntry<V>> {
        self.data.lock().expect("lock poisoned").get(key).cloned()
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) {
        self.data.lock().expect("lock poisoned").insert(key.clone(), entry);
    }

    fn len(&self) -> Option<u64> {
        Some(self.data.lock().expect("lock poisoned").len() as u64)
    }
}

#[tokio::test]
async fn is_empty_uses_len_when_available() {
    let cache: CacheWithLen<String, i32> = CacheWithLen::new();

    // Empty cache
    assert_eq!(cache.is_empty(), Some(true));
    assert_eq!(cache.len(), Some(0));

    // Add entry
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    assert_eq!(cache.is_empty(), Some(false));
    assert_eq!(cache.len(), Some(1));
}
