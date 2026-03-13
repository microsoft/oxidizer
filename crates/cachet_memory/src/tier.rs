// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory cache tier implementation.
//!
//! This module provides a high-performance concurrent in-memory cache tier
//! with automatic eviction and optional time-based expiration.

use cachet_tier::{CacheEntry, CacheTier, Error};
use moka::Expiry;
use moka::future::Cache;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::time::{Duration, Instant};
use thread_aware::{Arc, PerProcess, ThreadAware};

use crate::builder::InMemoryCacheBuilder;

/// A concurrent in-memory cache tier.
///
/// This cache provides:
/// - Concurrent access with high performance
/// - Automatic eviction based on capacity
/// - Thread-safe operations
///
/// # Examples
///
/// ```no_run
/// use cachet_memory::InMemoryCache;
/// use cachet_tier::{CacheEntry, CacheTier};
///
/// # async {
///
/// let cache = InMemoryCache::<String, i32>::new();
///
/// cache
///     .insert(&"key".to_string(), CacheEntry::new(42))
///     .await
///     .unwrap();
/// let value = cache.get(&"key".to_string()).await.unwrap();
/// assert_eq!(*value.unwrap().value(), 42);
/// # };
/// ```
#[derive(Debug, Clone, ThreadAware)]
pub struct InMemoryCache<K, V, H = RandomState>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    // TODO: Eventually we can support different strategies here.
    // For now we use a PerProcess cache since it supports concurrency.
    inner: Arc<Cache<K, CacheEntry<V>, H>, PerProcess>,
}

impl<K, V> Default for InMemoryCache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> InMemoryCache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Creates a new unbounded in-memory cache.
    ///
    /// The cache will use default eviction policy (`TinyLFU`).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Creates a new in-memory cache with a maximum capacity.
    ///
    /// Once the capacity is reached, entries will be evicted using
    /// the `TinyLFU` policy (combination of LRU eviction and LFU admission).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::with_capacity(1000);
    /// ```
    #[must_use]
    pub fn with_capacity(max_capacity: u64) -> Self {
        Self::builder().max_capacity(max_capacity).build()
    }

    /// Creates a new builder for configuring an in-memory cache.
    ///
    /// The builder provides access to additional configuration options
    /// such as time-to-live, time-to-idle, and initial capacity.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .max_capacity(1000)
    ///     .time_to_live(Duration::from_secs(300))
    ///     .time_to_idle(Duration::from_secs(60))
    ///     .build();
    /// ```
    #[cfg_attr(test, mutants::skip)] // Default::default() for InMemoryCacheBuilder calls new(), identical behavior
    #[must_use]
    pub fn builder() -> InMemoryCacheBuilder<K, V> {
        InMemoryCacheBuilder::new()
    }
}

impl<K, V, H> InMemoryCache<K, V, H>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    /// Constructs an `InMemoryCache` from a builder.
    ///
    /// This is called by `InMemoryCacheBuilder::build()` and should not
    /// be called directly by users.
    pub(crate) fn from_builder(builder: &InMemoryCacheBuilder<K, V, H>) -> Self {
        let mut moka_builder = Cache::builder();

        if let Some(capacity) = builder.max_capacity {
            moka_builder = moka_builder.max_capacity(capacity);
        }

        if let Some(capacity) = builder.initial_capacity {
            moka_builder = moka_builder.initial_capacity(capacity);
        }

        if let Some(ttl) = builder.time_to_live {
            moka_builder = moka_builder.time_to_live(ttl);
        }

        if let Some(tti) = builder.time_to_idle {
            moka_builder = moka_builder.time_to_idle(tti);
        }

        if let Some(name) = builder.name.as_deref() {
            moka_builder = moka_builder.name(name);
        }

        Self {
            inner: Arc::from_unaware(moka_builder.expire_after(EntryExpiry).build_with_hasher(builder.hasher.clone())),
        }
    }
}

impl<K, V, H> CacheTier<K, V> for InMemoryCache<K, V, H>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.inner.get(key).await)
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.inner.insert(key.clone(), entry).await;
        Ok(())
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.inner.invalidate(key).await;
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.invalidate_all();
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        Some(self.inner.entry_count())
    }
}

struct EntryExpiry;

impl<K, V> Expiry<K, CacheEntry<V>> for EntryExpiry {
    fn expire_after_create(&self, _key: &K, value: &CacheEntry<V>, _created_at: Instant) -> Option<Duration> {
        value.ttl()
    }

    fn expire_after_update(
        &self,
        _key: &K,
        value: &CacheEntry<V>,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        value.ttl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::time::SystemTime;

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn with_capacity_sets_max_capacity() {
        let cache = InMemoryCache::<String, i32>::with_capacity(100);
        assert_eq!(cache.inner.policy().max_capacity(), Some(100));
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn len_returns_nonzero_after_insert() {
        let cache = InMemoryCache::<String, i32>::new();
        futures::executor::block_on(async {
            cache.inner.insert("key".to_string(), CacheEntry::new(42)).await;
            cache.inner.run_pending_tasks().await;
        });
        assert!(cache.len().unwrap() > 0);
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn custom_hasher_get_insert_invalidate() {
        use std::collections::hash_map::RandomState;

        let cache = InMemoryCache::<String, i32>::builder()
            .hasher(RandomState::new())
            .max_capacity(100)
            .build();

        futures::executor::block_on(async {
            cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.unwrap();
            assert_eq!(*value.unwrap().value(), 42);

            assert_eq!(cache.len(), Some(1));

            cache.invalidate(&"key".to_string()).await.unwrap();
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.unwrap();
            assert!(value.is_none());
        });
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_max_capacity_sets_limit() {
        let expected_max_capacity = 100;
        let builder = InMemoryCacheBuilder::<String, i32>::new().max_capacity(expected_max_capacity);

        assert_eq!(builder.max_capacity, Some(expected_max_capacity));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_initial_capacity_sets_initial_capacity() {
        let expected_initial_capacity = 50;
        let builder = InMemoryCacheBuilder::<String, i32>::new().initial_capacity(expected_initial_capacity);

        assert_eq!(builder.initial_capacity, Some(50));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_time_to_live_sets_ttl() {
        let expected_ttl = Duration::from_secs(300);
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_live(expected_ttl);

        assert_eq!(builder.time_to_live, Some(expected_ttl));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_time_to_idle_sets_tti() {
        let expected_tti = Duration::from_secs(60);
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_idle(expected_tti);

        assert_eq!(builder.time_to_idle, Some(expected_tti));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_name_sets_cache_name() {
        let expected_name = "test-cache".to_string();
        let builder = InMemoryCacheBuilder::<String, i32>::new().name(expected_name);

        assert_eq!(builder.name.as_deref(), Some("test-cache"));
    }

    #[test]
    fn insert_and_get_returns_value() {
        let cache = InMemoryCache::<String, i32>::new();
        block_on(async {
            cache
                .insert(&"key".to_string(), CacheEntry::new(42))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.expect("Get should succeed");
            assert_eq!(*value.unwrap().value(), 42);
        })
    }

    #[test]
    fn get_returns_none_after_per_entry_ttl() {
        let cache = InMemoryCache::<String, i32>::new();
        block_on(async {
            cache
                .insert(&"key".to_string(), CacheEntry::expires_at(42, Duration::ZERO, SystemTime::now()))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.expect("Get should return none");
            assert!(value.is_none());
        });
    }

    #[test]
    fn get_returns_none_after_cache_ttl() {
        let cache = InMemoryCache::<String, i32>::builder().time_to_live(Duration::ZERO).build();
        block_on(async {
            cache
                .insert(&"key".to_string(), CacheEntry::new(42))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.expect("Get should return none");
            assert!(value.is_none());
        });
    }

    #[test]
    fn get_returns_none_after_cache_tti() {
        let cache = InMemoryCache::<String, i32>::builder().time_to_idle(Duration::ZERO).build();
        block_on(async {
            cache
                .insert(&"key".to_string(), CacheEntry::new(42))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.expect("Get should return none");
            assert!(value.is_none());
        });
    }

    #[test]
    fn invalidate_removes_entry() {
        let cache = InMemoryCache::<String, i32>::new();
        block_on(async {
            cache
                .insert(&"key".to_string(), CacheEntry::new(42))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            cache.invalidate(&"key".to_string()).await.expect("Invalidate should succeed");
            cache.inner.run_pending_tasks().await;

            let value = cache.get(&"key".to_string()).await.expect("Get should return none");
            assert!(value.is_none());
        });
    }

    #[test]
    fn clear_removes_all_entries() {
        let cache = InMemoryCache::<String, i32>::new();
        block_on(async {
            cache
                .insert(&"key1".to_string(), CacheEntry::new(42))
                .await
                .expect("Insert should succeed");
            cache
                .insert(&"key2".to_string(), CacheEntry::new(43))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            cache.clear().await.expect("Clear should succeed");
            cache.inner.run_pending_tasks().await;

            let value1 = cache.get(&"key1".to_string()).await.expect("Get should return none");
            let value2 = cache.get(&"key2".to_string()).await.expect("Get should return none");
            assert!(value1.is_none());
            assert!(value2.is_none());
        });
    }

    #[test]
    fn len_returns_correct_count() {
        let cache = InMemoryCache::<String, i32>::new();
        block_on(async {
            assert_eq!(cache.len(), Some(0));

            cache
                .insert(&"key1".to_string(), CacheEntry::new(42))
                .await
                .expect("Insert should succeed");
            cache
                .insert(&"key2".to_string(), CacheEntry::new(43))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            assert_eq!(cache.len(), Some(2));

            cache.invalidate(&"key1".to_string()).await.expect("Invalidate should succeed");
            cache.inner.run_pending_tasks().await;

            assert_eq!(cache.len(), Some(1));

            cache.clear().await.expect("Clear should succeed");
            cache.inner.run_pending_tasks().await;

            assert_eq!(cache.len(), Some(0));
        });
    }

    #[test]
    fn max_capacity_evicts_at_capacity() {
        let capacity = 5;
        let cache = InMemoryCache::<String, i32>::builder().max_capacity(capacity).build();
        block_on(async {
            for i in 0..=capacity {
                cache
                    .insert(&format!("key{}", i), CacheEntry::new(i as i32))
                    .await
                    .expect("Insert should succeed");
            }
            cache.inner.run_pending_tasks().await;

            // Insert one more entry to trigger eviction
            cache
                .insert(&format!("key{}", capacity), CacheEntry::new(capacity as i32))
                .await
                .expect("Insert should succeed");
            cache.inner.run_pending_tasks().await;

            // The cache should only have max_capacity entries
            assert_eq!(cache.len(), Some(capacity));
        });
    }
}
