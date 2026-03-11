// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory cache tier implementation.
//!
//! This module provides a high-performance concurrent in-memory cache tier
//! with automatic eviction and optional time-based expiration.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};

use cachet_tier::{CacheEntry, CacheTier, Error};
use moka::future::Cache;
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
/// cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
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
    /// use cachet_memory::InMemoryCache;
    /// use std::time::Duration;
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
            inner: Arc::from_unaware(moka_builder.build_with_hasher(builder.hasher.clone())),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
