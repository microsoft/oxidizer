// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `CacheTier` trait default implementations.

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(feature = "test-util")]
use cachet_tier::MockCache;
use cachet_tier::{CacheEntry, CacheTier, DynamicCache, Error};

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
async fn minimal_cachet_get_miss() {
    let cache = MinimalCache::<String, i32>::new();
    let result: Option<CacheEntry<i32>> = cache.get(&"key".to_string()).await.expect("error on get");
    assert!(result.is_none());
}

#[tokio::test]
async fn minimal_cachet_get_hit() {
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
async fn default_is_empty_returns_none_when_len_is_none() {
    let cache: MinimalCache<String, i32> = MinimalCache::new();
    // is_empty delegates to len(); since len() returns None, is_empty() should too
    assert!(cache.is_empty().is_none());
}

/// Cache that implements `len()` so we can test `is_empty()` default derivation
struct SizedCache<K, V> {
    data: Mutex<HashMap<K, CacheEntry<V>>>,
}

impl<K, V> SizedCache<K, V> {
    fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl<K, V> CacheTier<K, V> for SizedCache<K, V>
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
async fn is_empty_returns_true_for_empty_cache() {
    let cache = SizedCache::<String, i32>::new();
    assert_eq!(cache.is_empty(), Some(true));
}

#[tokio::test]
async fn is_empty_returns_false_for_non_empty_cache() {
    let cache = SizedCache::<String, i32>::new();
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert_eq!(cache.is_empty(), Some(false));
}

// MockCache tests

#[cfg(feature = "test-util")]
#[test]
fn mock_cache_len_empty() {
    let cache = MockCache::<String, i32>::new();
    assert_eq!(cache.len(), Some(0));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_len_after_insert() {
    let cache = MockCache::<String, i32>::new();
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert_eq!(cache.len(), Some(1));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_len_after_multiple_inserts() {
    let cache = MockCache::<String, i32>::new();
    cache.insert(&"a".to_string(), CacheEntry::new(1)).await.unwrap();
    cache.insert(&"b".to_string(), CacheEntry::new(2)).await.unwrap();
    assert_eq!(cache.len(), Some(2));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_is_empty_delegates_to_len() {
    let cache = MockCache::<String, i32>::new();
    assert_eq!(cache.is_empty(), Some(true));

    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert_eq!(cache.is_empty(), Some(false));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_entry_count() {
    let cache = MockCache::<String, i32>::new();
    assert_eq!(cache.entry_count(), 0);

    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert_eq!(cache.entry_count(), 1);
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_contains_key() {
    let cache = MockCache::<String, i32>::new();
    assert!(!cache.contains_key(&"key".to_string()));

    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert!(cache.contains_key(&"key".to_string()));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_clear_failures() {
    let cache = MockCache::<String, i32>::new();
    cache.fail_when(|_| true);
    cache.get(&"key".to_string()).await.unwrap_err();

    cache.clear_failures();
    cache.get(&"key".to_string()).await.unwrap();
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_operations_recording() {
    use cachet_tier::CacheOp;

    let cache = MockCache::<String, i32>::new();
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    cache.get(&"key".to_string()).await.unwrap();

    let ops = cache.operations();
    assert_eq!(ops.len(), 2);
    assert!(matches!(&ops[0], CacheOp::Insert { .. }));
    assert!(matches!(&ops[1], CacheOp::Get(_)));

    cache.clear_operations();
    assert!(cache.operations().is_empty());
}

#[cfg(feature = "test-util")]
#[test]
fn mock_cache_debug_contains_name() {
    let cache = MockCache::<String, i32>::new();
    let debug = format!("{cache:?}");
    assert!(debug.contains("MockCache"));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_clone_shares_state() {
    let cache = MockCache::<String, i32>::new();
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

    let cloned = cache.clone();
    let entry = cloned.get(&"key".to_string()).await.unwrap().unwrap();
    assert_eq!(*entry.value(), 42);
}

#[cfg(feature = "test-util")]
#[test]
fn mock_cache_default_creates_empty() {
    let cache = MockCache::<String, i32>::default();
    assert_eq!(cache.len(), Some(0));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_with_data_prepopulates() {
    let mut data = HashMap::new();
    data.insert("key".to_string(), CacheEntry::new(42));
    let cache = MockCache::with_data(data);
    let entry = cache.get(&"key".to_string()).await.unwrap().unwrap();
    assert_eq!(*entry.value(), 42);
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_invalidate_removes_entry() {
    let cache = MockCache::<String, i32>::new();
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert!(cache.contains_key(&"key".to_string()));

    cache.invalidate(&"key".to_string()).await.unwrap();
    assert!(!cache.contains_key(&"key".to_string()));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn mock_cache_clear_removes_all() {
    let cache = MockCache::<String, i32>::new();
    cache.insert(&"a".to_string(), CacheEntry::new(1)).await.unwrap();
    cache.insert(&"b".to_string(), CacheEntry::new(2)).await.unwrap();
    assert_eq!(cache.entry_count(), 2);

    cache.clear().await.unwrap();
    assert_eq!(cache.entry_count(), 0);
}

// DynamicCache tests

#[cfg(feature = "test-util")]
#[tokio::test]
async fn dynamic_cache_debug() {
    let cache = MockCache::<String, i32>::new();
    let dynamic = DynamicCache::new(cache);
    let debug = format!("{dynamic:?}");
    assert!(debug.contains("DynamicCache"));
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn dynamic_cache_clone_shares_state() {
    let cache = MockCache::<String, i32>::new();
    let dynamic = DynamicCache::new(cache);
    let clone = dynamic.clone();

    dynamic.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

    let entry = clone.get(&"key".to_string()).await.unwrap().unwrap();
    assert_eq!(*entry.value(), 42);
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn dynamic_cache_invalidate() {
    let cache = MockCache::<String, i32>::new();
    let dynamic = DynamicCache::new(cache);

    dynamic.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    dynamic.invalidate(&"key".to_string()).await.unwrap();

    assert!(dynamic.get(&"key".to_string()).await.unwrap().is_none());
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn dynamic_cache_clear() {
    let cache = MockCache::<String, i32>::new();
    let dynamic = DynamicCache::new(cache);

    dynamic.insert(&"a".to_string(), CacheEntry::new(1)).await.unwrap();
    dynamic.insert(&"b".to_string(), CacheEntry::new(2)).await.unwrap();

    dynamic.clear().await.unwrap();

    assert!(dynamic.get(&"a".to_string()).await.unwrap().is_none());
    assert!(dynamic.get(&"b".to_string()).await.unwrap().is_none());
}

#[cfg(feature = "test-util")]
#[tokio::test]
async fn dynamic_cache_len() {
    let cache = MockCache::<String, i32>::new();
    let dynamic = DynamicCache::new(cache);

    assert_eq!(dynamic.len(), Some(0));
    dynamic.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
    assert_eq!(dynamic.len(), Some(1));
}
