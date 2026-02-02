// Copyright (c) Microsoft Corporation.

//! Wrapper that adds telemetry and TTL to cache tiers.
//!
//! This module provides the `CacheWrapper` type that decorates any `CacheTier`
//! implementation with automatic telemetry recording and TTL expiration handling.

use std::{hash::Hash, marker::PhantomData, time::Duration};

use tick::Clock;

use crate::telemetry::{CacheEvent, CacheOperation, CacheTelemetry};
use crate::{
    CacheEntry, Error,
    cache::CacheName,
    telemetry::ext::{CacheTelemetryExt, ClockExt},
};

use cachelon_tier::CacheTier;

/// Wraps a cache tier with telemetry and TTL expiration.
///
/// This decorator adds cross-cutting concerns to cache tiers:
/// - Automatic telemetry recording for all operations
/// - TTL-based expiration checking on retrieval
/// - Timestamp management on insertion
///
/// It implements `CacheTier` so it can be composed with `FallbackCache`.
///
/// # Examples
///
/// This type is typically created by the cache builder rather than directly:
///
/// ```
/// use cachelon::Cache;
/// use tick::Clock;
/// use std::time::Duration;
///
/// let clock = Clock::new_frozen();
/// let cache = Cache::builder::<String, i32>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .build();
/// ```
#[derive(Debug)]
pub struct CacheWrapper<K, V, S> {
    pub(crate) name: CacheName,
    pub(crate) inner: S,
    pub(crate) clock: Clock,
    pub(crate) ttl: Option<Duration>,
    pub(crate) telemetry: Option<CacheTelemetry>,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, S> CacheWrapper<K, V, S> {
    pub(crate) fn new(name: CacheName, inner: S, clock: Clock, ttl: Option<Duration>, telemetry: Option<CacheTelemetry>) -> Self {
        Self {
            name,
            inner,
            clock,
            ttl,
            telemetry,
            _phantom: PhantomData,
        }
    }

    /// Returns the name of this cache tier for telemetry identification.
    #[must_use]
    pub fn name(&self) -> CacheName {
        self.name
    }

    /// Returns a reference to the wrapped storage tier.
    #[must_use]
    pub fn inner(&self) -> &S {
        &self.inner
    }
}

impl<K, V, S> CacheWrapper<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
    S: CacheTier<K, V> + Send + Sync,
{
    fn is_expired(&self, entry: &CacheEntry<V>) -> bool {
        // Per-entry TTL takes precedence over tier-level TTL
        let ttl = entry.ttl().or(self.ttl);
        if let Some(ttl) = ttl {
            entry.cached_at().is_some_and(|cached_at| cached_at.elapsed() > ttl)
        } else {
            false
        }
    }

    fn handle_get_result(&self, value: Option<CacheEntry<V>>, duration: Duration) -> Option<CacheEntry<V>> {
        if let Some(entry) = value {
            if self.is_expired(&entry) {
                self.telemetry.record(self.name, CacheOperation::Get, CacheEvent::Expired, duration);
                None
            } else {
                self.telemetry.record(self.name, CacheOperation::Get, CacheEvent::Hit, duration);
                Some(entry)
            }
        } else {
            self.telemetry.record(self.name, CacheOperation::Get, CacheEvent::Miss, duration);
            None
        }
    }
}

impl<K, V, S> CacheTier<K, V> for CacheWrapper<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
    S: CacheTier<K, V> + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let timed = self.clock.timed_async(self.inner.get(key)).await;
        match timed.result {
            Ok(value) => Ok(self.handle_get_result(value, timed.duration)),
            Err(e) => {
                self.telemetry
                    .record(self.name, CacheOperation::Get, CacheEvent::Error, timed.duration);
                Err(e)
            }
        }
    }

    async fn insert(&self, key: &K, mut entry: CacheEntry<V>) -> Result<(), Error> {
        entry.set_cached_at(self.clock.instant());
        let timed = self.clock.timed_async(self.inner.insert(key, entry)).await;
        match &timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Insert, CacheEvent::Inserted, timed.duration);
                if let Some(size) = self.inner.len() {
                    self.telemetry.record_size(self.name, size);
                }
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Insert, CacheEvent::Error, timed.duration);
            }
        }
        timed.result
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let timed = self.clock.timed_async(self.inner.invalidate(key)).await;
        match &timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Invalidate, CacheEvent::Invalidated, timed.duration);
                if let Some(size) = self.inner.len() {
                    self.telemetry.record_size(self.name, size);
                }
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Invalidate, CacheEvent::Error, timed.duration);
            }
        }
        timed.result
    }

    async fn clear(&self) -> Result<(), Error> {
        let timed = self.clock.timed_async(self.inner.clear()).await;
        match &timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Clear, CacheEvent::Ok, timed.duration);
                if let Some(size) = self.inner.len() {
                    self.telemetry.record_size(self.name, size);
                }
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Clear, CacheEvent::Error, timed.duration);
            }
        }
        timed.result
    }

    fn len(&self) -> Option<u64> {
        self.inner.len()
    }

    fn is_empty(&self) -> Option<bool> {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cachelon_memory::InMemoryCache;

    // Public API tests (via Cache) moved to tests/wrapper.rs
    // Tests below access internal CacheWrapper construction or private methods

    #[test]
    fn wrapper_inner() {
        let clock = Clock::new_frozen();
        let inner = InMemoryCache::<String, i32>::new();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, None);
        let _inner_ref = wrapper.inner();
    }

    #[test]
    fn wrapper_is_expired_with_no_ttl() {
        let clock = Clock::new_frozen();
        let inner = InMemoryCache::<String, i32>::new();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, None);

        // Entry without TTL should not be expired
        let entry = CacheEntry::new(42);
        assert!(!wrapper.is_expired(&entry));
    }

    #[test]
    fn wrapper_is_expired_without_cached_at() {
        let clock = Clock::new_frozen();
        let inner = InMemoryCache::<String, i32>::new();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, Some(Duration::from_secs(60)), None);

        // Entry without cached_at should not be expired (even with tier TTL)
        let entry = CacheEntry::new(42);
        assert!(!wrapper.is_expired(&entry));
    }

    #[test]
    fn wrapper_entry_ttl_takes_precedence_over_tier_ttl() {
        let clock = Clock::new_frozen();
        let inner = InMemoryCache::<String, i32>::new();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new(
            "test",
            inner,
            clock,
            Some(Duration::from_secs(60)), // tier TTL: 60 seconds
            None,
        );

        // Entry with per-entry TTL should use entry TTL
        let mut entry = CacheEntry::with_ttl(42, Duration::from_secs(120)); // entry TTL: 120 seconds
        entry.set_cached_at(std::time::Instant::now());

        // Entry TTL is longer than tier TTL, so entry should not be expired
        assert!(!wrapper.is_expired(&entry));
    }
}
