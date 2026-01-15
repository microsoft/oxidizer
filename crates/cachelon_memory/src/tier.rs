//! In-memory cache implementation using moka.
//!
//! This module provides an in-memory cache tier backed by the moka crate,
//! which offers high-performance concurrent caching with eviction policies.

use std::hash::Hash;

use cachelon_tier::{CacheEntry, CacheTier, Error};
use moka::future::Cache;
use thread_aware::{Arc, PerProcess, ThreadAware};

use crate::builder::InMemoryCacheBuilder;

/// An in-memory cache tier backed by moka.
///
/// This cache provides:
/// - Concurrent access with high performance
/// - Automatic eviction based on capacity
/// - Thread-safe operations
///
/// # Examples
///
/// ```
/// use cachelon_memory::InMemoryCache;
/// use cachelon_tier::{CacheEntry, CacheTier};
/// # futures::executor::block_on(async {
///
/// let cache = InMemoryCache::<String, i32>::new();
///
/// cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
/// let value = cache.get(&"key".to_string()).await;
/// assert_eq!(*value.unwrap().value(), 42);
/// # });
/// ```
#[derive(Debug, Clone, ThreadAware)]
pub struct InMemoryCache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    // TODO: Eventually we can support different strategies here.
    // For now we will use Moka as a PerProcess cache since it supports concurrency.
    inner: Arc<Cache<K, CacheEntry<V>>, PerProcess>,
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
    /// ```
    /// use cachelon_memory::InMemoryCache;
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
    /// ```
    /// use cachelon_memory::InMemoryCache;
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
    /// ```
    /// use cachelon_memory::InMemoryCache;
    /// use std::time::Duration;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .max_capacity(1000)
    ///     .time_to_live(Duration::from_secs(300))
    ///     .time_to_idle(Duration::from_secs(60))
    ///     .build();
    /// ```
    #[must_use]
    pub fn builder() -> InMemoryCacheBuilder<K, V> {
        InMemoryCacheBuilder::new()
    }

    /// Constructs an `InMemoryCache` from a builder.
    ///
    /// This is called by `InMemoryCacheBuilder::build()` and should not
    /// be called directly by users.
    pub(crate) fn from_builder(builder: &InMemoryCacheBuilder<K, V>) -> Self {
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
            inner: Arc::from_unaware(moka_builder.build()),
        }
    }
}

impl<K, V> CacheTier<K, V> for InMemoryCache<K, V>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn get(&self, key: &K) -> Option<CacheEntry<V>> {
        self.inner.get(key).await
    }

    async fn try_get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.inner.get(key).await)
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) {
        self.inner.insert(key.clone(), entry.clone()).await;
    }

    async fn try_insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.inner.insert(key.clone(), entry.clone()).await;
        Ok(())
    }

    async fn invalidate(&self, key: &K) {
        self.inner.invalidate(key).await;
    }

    async fn try_invalidate(&self, key: &K) -> Result<(), Error> {
        self.inner.invalidate(key).await;
        Ok(())
    }

    async fn clear(&self) {
        self.inner.invalidate_all();
    }

    async fn try_clear(&self) -> Result<(), Error> {
        self.inner.invalidate_all();
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        Some(self.inner.entry_count())
    }
}
