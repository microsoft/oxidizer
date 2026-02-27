// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache entry type with value and metadata.

use std::{
    ops::Deref,
    time::{Duration, SystemTime},
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
/// // Simple entry with just a value (no expiration)
/// let entry = CacheEntry::new(42);
/// assert_eq!(*entry.value(), 42);
///
/// // Entry that expires after a duration from insert time
/// let entry = CacheEntry::expires_after("data".to_string(), Duration::from_secs(60));
/// assert_eq!(entry.ttl(), Some(Duration::from_secs(60)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry<V> {
    value: V,
    cached_at: Option<SystemTime>,
    /// Per-entry TTL override. If set, takes precedence over cache-level TTL.
    ttl: Option<Duration>,
}

impl<V> CacheEntry<V> {
    /// Creates a new cache entry with the given value.
    ///
    /// The entry will not expire unless a tier-level TTL is configured.
    pub fn new(value: V) -> Self {
        Self {
            value,
            cached_at: None,
            ttl: None,
        }
    }

    /// Creates a cache entry that expires after the given duration from insert time.
    ///
    /// The timestamp will be set by the cache when the entry is inserted.
    /// The per-entry TTL takes precedence over any tier-level TTL.
    pub fn expires_after(value: V, ttl: Duration) -> Self {
        Self {
            value,
            cached_at: None,
            ttl: Some(ttl),
        }
    }

    /// Creates a cache entry that expires at `cached_at + ttl`.
    ///
    /// This is primarily useful for testing expiration logic with controlled
    /// timestamps, or if you need to construct entries with pre-determined
    /// expiration times.
    pub fn expires_at(value: V, ttl: Duration, cached_at: SystemTime) -> Self {
        Self {
            value,
            cached_at: Some(cached_at),
            ttl: Some(ttl),
        }
    }

    /// Returns the timestamp when this entry was cached.
    ///
    /// Returns `None` if the entry hasn't been inserted into a cache yet,
    /// or was created without [`expires_at`](Self::expires_at).
    #[must_use]
    pub fn cached_at(&self) -> Option<SystemTime> {
        self.cached_at
    }

    /// Sets the cache timestamp if not already set.
    ///
    /// Called automatically by the cache during insertion. If the entry
    /// was created with [`expires_at`](Self::expires_at), the existing
    /// timestamp is preserved.
    pub fn ensure_cached_at(&mut self, cached_at: SystemTime) {
        if self.cached_at.is_none() {
            self.cached_at = Some(cached_at);
        }
    }

    /// Returns the per-entry TTL override.
    ///
    /// When set, this takes precedence over any tier-level TTL configured
    /// on the cache.
    #[must_use]
    pub fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    /// Sets a per-entry TTL that overrides the tier-level TTL.
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
