// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for configuring in-memory caches.
//!
//! This module provides a builder API for `InMemoryCache` that abstracts
//! the underlying moka configuration, providing a stable API surface
//! without exposing moka's types.

use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

use crate::tier::InMemoryCache;

/// Builder for configuring an `InMemoryCache`.
///
/// This builder provides a stable API for common cache configuration
/// options without exposing the underlying moka cache implementation.
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
///     .initial_capacity(100)
///     .name("my-cache")
///     .build();
/// ```
#[derive(Debug)]
pub struct InMemoryCacheBuilder<K, V> {
    pub(crate) max_capacity: Option<u64>,
    pub(crate) initial_capacity: Option<usize>,
    pub(crate) time_to_live: Option<Duration>,
    pub(crate) time_to_idle: Option<Duration>,
    pub(crate) name: Option<String>,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V> Default for InMemoryCacheBuilder<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> InMemoryCacheBuilder<K, V> {
    /// Creates a new builder with default settings.
    ///
    /// The default configuration creates an unbounded cache with `TinyLFU`
    /// eviction policy and no time-based expiration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_capacity: None,
            initial_capacity: None,
            time_to_live: None,
            time_to_idle: None,
            name: None,
            _phantom: PhantomData,
        }
    }

    /// Sets the maximum capacity of the cache.
    ///
    /// Once the capacity is reached, entries will be evicted to make room
    /// for new entries using the `TinyLFU` eviction policy (combination of
    /// LRU eviction and LFU admission).
    ///
    /// If not set, the cache will be unbounded (limited only by available memory).
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .max_capacity(10_000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn max_capacity(mut self, capacity: u64) -> Self {
        self.max_capacity = Some(capacity);
        self
    }

    /// Sets the initial capacity (pre-allocation hint) for the cache.
    ///
    /// This can improve performance by avoiding reallocations during
    /// initial population. The cache may still grow beyond this size.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .initial_capacity(100)
    ///     .max_capacity(10_000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn initial_capacity(mut self, capacity: usize) -> Self {
        self.initial_capacity = Some(capacity);
        self
    }

    /// Sets the time-to-live (TTL) for all entries.
    ///
    /// Entries will expire after this duration from insertion, regardless
    /// of access patterns. This is enforced at the cache tier level and is
    /// independent of any per-entry TTL set via `CacheEntry::with_ttl()`.
    ///
    /// Expired entries are removed lazily during cache operations and
    /// automatically in the background using hierarchical timer wheels.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_memory::InMemoryCache;
    /// use std::time::Duration;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .time_to_live(Duration::from_secs(300))
    ///     .build();
    /// ```
    #[must_use]
    pub fn time_to_live(mut self, duration: Duration) -> Self {
        self.time_to_live = Some(duration);
        self
    }

    /// Sets the time-to-idle (TTI) for all entries.
    ///
    /// Entries will expire after this duration of inactivity (no reads or writes).
    /// The timer is reset on each access (get or insert operation).
    ///
    /// Expired entries are removed lazily during cache operations and
    /// automatically in the background using hierarchical timer wheels.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_memory::InMemoryCache;
    /// use std::time::Duration;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .time_to_idle(Duration::from_secs(60))
    ///     .build();
    /// ```
    #[must_use]
    pub fn time_to_idle(mut self, duration: Duration) -> Self {
        self.time_to_idle = Some(duration);
        self
    }

    /// Sets a name for the cache.
    ///
    /// This name may appear in logs or debugging output from the
    /// underlying cache implementation.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .name("user-cache")
    ///     .build();
    /// ```
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Builds the configured `InMemoryCache`.
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
    ///     .build();
    /// ```
    #[must_use]
    pub fn build(self) -> InMemoryCache<K, V>
    where
        K: Hash + Eq + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        InMemoryCache::from_builder(&self)
    }
}
