// Copyright (c) Microsoft Corporation.

use std::{
    ops::Deref,
    time::{Duration, Instant},
};

/// A cached value with associated metadata.
///
/// `CacheEntry` wraps a value with optional TTL and timestamp information.
/// The cache system uses this metadata for expiration tracking and telemetry.
///
/// # Examples
///
/// ```
/// use cachelon_tier::CacheEntry;
/// use std::time::Duration;
///
/// // Simple entry with just a value
/// let entry = CacheEntry::new(42);
/// assert_eq!(*entry.value(), 42);
///
/// // Entry with per-entry TTL
/// let entry = CacheEntry::with_ttl("data".to_string(), Duration::from_secs(60));
/// assert_eq!(entry.ttl(), Some(Duration::from_secs(60)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry<V> {
    value: V,
    cached_at: Option<Instant>,
    /// Per-entry TTL override. If set, takes precedence over cache-level TTL.
    ttl: Option<Duration>,
}

impl<V> CacheEntry<V> {
    /// Creates a new cache entry with the given value.
    ///
    /// The timestamp will be set by the cache when the entry is inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_tier::CacheEntry;
    ///
    /// let entry = CacheEntry::new(42);
    /// assert_eq!(*entry.value(), 42);
    /// assert!(entry.cached_at().is_none());
    /// ```
    pub fn new(value: V) -> Self {
        Self {
            value,
            cached_at: None,
            ttl: None,
        }
    }

    /// Creates a new cache entry with a per-entry TTL.
    ///
    /// The per-entry TTL takes precedence over any tier-level TTL.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_tier::CacheEntry;
    /// use std::time::Duration;
    ///
    /// let entry = CacheEntry::with_ttl(42, Duration::from_secs(300));
    /// assert_eq!(entry.ttl(), Some(Duration::from_secs(300)));
    /// ```
    pub fn with_ttl(value: V, ttl: Duration) -> Self {
        Self {
            value,
            cached_at: None,
            ttl: Some(ttl),
        }
    }

    /// Creates a new cache entry with an explicit timestamp.
    ///
    /// This is typically used when recreating entries from persistent storage.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_tier::CacheEntry;
    /// use std::time::Instant;
    ///
    /// let now = Instant::now();
    /// let entry = CacheEntry::with_cached_at(42, now);
    /// assert_eq!(entry.cached_at(), Some(now));
    /// ```
    pub fn with_cached_at(value: V, cached_at: Instant) -> Self {
        Self {
            value,
            cached_at: Some(cached_at),
            ttl: None,
        }
    }

    /// Returns the timestamp when this entry was cached.
    ///
    /// Returns `None` if the entry hasn't been inserted yet or was created
    /// without a timestamp.
    #[must_use]
    pub fn cached_at(&self) -> Option<Instant> {
        self.cached_at
    }

    /// Sets the timestamp when this entry was cached.
    ///
    /// This is typically called by the cache implementation when inserting.
    pub fn set_cached_at(&mut self, cached_at: Instant) {
        self.cached_at = Some(cached_at);
    }

    /// Returns the per-entry TTL, if set.
    ///
    /// Per-entry TTL takes precedence over tier-level TTL.
    #[must_use]
    pub fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    /// Sets the per-entry TTL.
    ///
    /// This overrides any tier-level TTL for this specific entry.
    pub fn set_ttl(&mut self, ttl: Duration) {
        self.ttl = Some(ttl);
    }

    /// Consumes the entry and returns the inner value.
    #[must_use]
    pub fn into_value(self) -> V {
        self.value
    }

    /// Returns a reference to the cached value.
    #[must_use]
    pub fn value(&self) -> &V {
        &self.value
    }
}

impl<V> Deref for CacheEntry<V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<V> From<V> for CacheEntry<V> {
    fn from(value: V) -> Self {
        Self::new(value)
    }
}
