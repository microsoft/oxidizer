// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for configuring in-memory caches.
//!
//! This module provides a builder API for `InMemoryCache` that abstracts
//! the underlying cache configuration, providing a stable API surface
//! without exposing implementation details.

use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::time::Duration;

use foldhash::fast::RandomState;

use crate::policy::EvictionPolicy;
use crate::tier::InMemoryCache;

/// Builder for configuring an `InMemoryCache`.
///
/// This builder provides a stable API for common cache configuration
/// options without exposing the underlying cache implementation.
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
///     .initial_capacity(100)
///     .name("my-cache")
///     .build();
/// ```
#[derive(Debug)]
pub struct InMemoryCacheBuilder<K, V, H = RandomState> {
    pub(crate) max_capacity: Option<u64>,
    pub(crate) initial_capacity: Option<usize>,
    pub(crate) time_to_live: Option<Duration>,
    pub(crate) time_to_idle: Option<Duration>,
    pub(crate) name: Option<&'static str>,
    pub(crate) eviction_policy: EvictionPolicy,
    pub(crate) hasher: H,
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
            eviction_policy: EvictionPolicy::default(),
            hasher: RandomState::default(),
            _phantom: PhantomData,
        }
    }
}

impl<K, V, H> InMemoryCacheBuilder<K, V, H> {
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
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
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
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
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
    /// independent of any per-entry TTL set via `CacheEntry::expires_after()`.
    ///
    /// Expired entries are removed lazily during cache operations and
    /// automatically in the background using hierarchical timer wheels.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet_memory::InMemoryCache;
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
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet_memory::InMemoryCache;
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
    /// Requires `&'static str` for consistency with the outer cache builder,
    /// where the name is embedded in every telemetry event. A static reference
    /// avoids cloning on each cache operation. In practice, cache names are
    /// always string literals.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .name("user-cache")
    ///     .build();
    /// ```
    #[must_use]
    pub fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Sets the eviction policy for the cache.
    ///
    /// Controls how entries are chosen for eviction when the cache reaches its
    /// maximum capacity. Defaults to [`EvictionPolicy::tiny_lfu()`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::{InMemoryCacheBuilder, policy::EvictionPolicy};
    ///
    /// let cache = InMemoryCacheBuilder::<String, i32>::new()
    ///     .max_capacity(1000)
    ///     .eviction_policy(EvictionPolicy::lru())
    ///     .build()
    ///     .expect("Failed to build cache");
    /// ```
    #[must_use]
    pub fn eviction_policy(mut self, policy: EvictionPolicy) -> Self {
        self.eviction_policy = policy;
        self
    }

    /// Sets a custom hash builder for the cache.
    ///
    /// By default, the cache uses [`foldhash::fast::RandomState`] for high-performance
    /// hashing. Use this method to provide an alternative hasher implementation.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::collections::hash_map::RandomState;
    ///
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .with_hasher(RandomState::new())
    ///     .max_capacity(1000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_hasher<H2>(self, hasher: H2) -> InMemoryCacheBuilder<K, V, H2> {
        InMemoryCacheBuilder {
            max_capacity: self.max_capacity,
            initial_capacity: self.initial_capacity,
            time_to_live: self.time_to_live,
            time_to_idle: self.time_to_idle,
            name: self.name,
            eviction_policy: self.eviction_policy,
            hasher,
            _phantom: PhantomData,
        }
    }

    /// Builds the configured `InMemoryCache`.
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
    ///     .build();
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid
    /// Configuration is invalid when:
    /// - Initial capacity is greater than max capacity (if max capacity is set)
    /// - Time-to-idle is greater than time-to-live (if both are set)
    pub fn build(self) -> Result<InMemoryCache<K, V, H>, ValidationError>
    where
        K: Hash + Eq + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        H: BuildHasher + Clone + Send + Sync + 'static,
    {
        self.validate()?;
        Ok(InMemoryCache::from_builder(self))
    }

    fn validate(&self) -> Result<(), ValidationError> {
        ValidationError::invalid_capacity(self.max_capacity, self.initial_capacity).map_or(Ok(()), Err)?;
        ValidationError::invalid_time_to(self.time_to_live, self.time_to_idle).map_or(Ok(()), Err)?;
        Ok(())
    }

    pub(crate) fn build_unchecked(self) -> InMemoryCache<K, V, H>
    where
        K: Hash + Eq + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        H: BuildHasher + Clone + Send + Sync + 'static,
    {
        InMemoryCache::from_builder(self)
    }
}

#[ohno::error]
#[display("invalid cache configuration: {reason}")]
pub struct ValidationError {
    reason: String,
}

impl ValidationError {
    fn invalid_capacity(max_capacity: Option<u64>, initial_capacity: Option<usize>) -> Option<Self> {
        let max = max_capacity?;
        let init = initial_capacity?;
        (init as u64 > max).then(|| Self::new(format!("initial_capacity ({init}) exceeds max_capacity ({max})")))
    }

    fn invalid_time_to(time_to_live: Option<Duration>, time_to_idle: Option<Duration>) -> Option<Self> {
        let time_to_idle = time_to_idle?;
        let time_to_live = time_to_live?;
        (time_to_idle > time_to_live)
            .then(|| Self::new(format!("time to idle ({time_to_idle:?}) exceeds time to live ({time_to_live:?}).")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_capacity_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().max_capacity(100);
        assert_eq!(builder.max_capacity, Some(100));
    }

    #[test]
    fn initial_capacity_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().initial_capacity(50);
        assert_eq!(builder.initial_capacity, Some(50));
    }

    #[test]
    fn time_to_live_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_live(Duration::from_secs(300));
        assert_eq!(builder.time_to_live, Some(Duration::from_secs(300)));
    }

    #[test]
    fn time_to_idle_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_idle(Duration::from_secs(60));
        assert_eq!(builder.time_to_idle, Some(Duration::from_secs(60)));
    }

    #[test]
    fn name_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().name("test");
        assert_eq!(builder.name, Some("test"));
    }

    #[test]
    fn build_max_capacity_lt_initial_capacity_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(101)
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: initial_capacity (101) exceeds max_capacity (100)"
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_max_capacity_eq_initial_capacity_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(100)
            .build();
        result.unwrap();
    }

    #[test]
    fn build_ttl_less_than_tti_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_secs(60))
            .time_to_idle(Duration::from_secs(120))
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: time to idle (120s) exceeds time to live (60s)."
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_ttl_eq_tti_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_secs(60))
            .time_to_idle(Duration::from_secs(60))
            .build();
        result.unwrap();
    }

    #[test]
    fn build_eviction_policy_stores_value() {
        let policy = EvictionPolicy::lru();
        let builder = InMemoryCacheBuilder::<String, i32>::new().eviction_policy(policy.clone());
        assert_eq!(builder.eviction_policy, policy);
    }
}
