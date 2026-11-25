// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wrapper that adds telemetry and TTL to cache tiers.
//!
//! This module provides the `CacheWrapper` type that decorates any `CacheTier`
//! implementation with automatic telemetry recording and TTL expiration handling.

use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

use cachet_tier::{CacheTier, InsertOutcome, SizeError};
use tick::Clock;

use crate::cache::CacheName;
use crate::telemetry::CacheTelemetry;
use crate::{CacheEntry, Error, InsertPolicy};

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
/// ```no_run
/// use std::time::Duration;
///
/// use cachet::Cache;
/// use tick::Clock;
///
/// let clock = Clock::new_tokio();
/// let cache = Cache::builder::<String, i32>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .build();
/// ```
#[derive(Debug)]
pub(crate) struct CacheWrapper<K, V, CT> {
    pub(crate) name: CacheName,
    pub(crate) inner: CT,
    pub(crate) clock: Clock,
    pub(crate) ttl: Option<Duration>,
    pub(crate) telemetry: CacheTelemetry,
    pub(crate) policy: InsertPolicy<V>,
    pub(crate) fallback: bool,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, CT> CacheWrapper<K, V, CT> {
    pub(crate) fn new(
        name: CacheName,
        inner: CT,
        clock: Clock,
        ttl: Option<Duration>,
        telemetry: CacheTelemetry,
        policy: InsertPolicy<V>,
        fallback: bool,
    ) -> Self {
        Self {
            name,
            inner,
            clock,
            ttl,
            telemetry,
            policy,
            fallback,
            _phantom: PhantomData,
        }
    }
}

impl<K, V, CT> CacheWrapper<K, V, CT>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
    CT: CacheTier<K, V> + Send + Sync,
{
    fn is_expired(&self, entry: &CacheEntry<V>) -> bool {
        // Per-entry TTL takes precedence over tier-level TTL
        let ttl = entry.ttl().or(self.ttl);
        if let Some(ttl) = ttl {
            match entry.cached_at() {
                Some(cached_at) => match self.clock.system_time().duration_since(cached_at) {
                    Ok(elapsed) => elapsed > ttl,
                    Err(_) => true, // If the system time went backwards, consider it expired
                },
                None => true, // TODO: If have no cached_at timestamp, something went wrong; with TTL treat as expired?
            }
        } else {
            false
        }
    }

    fn handle_get_result(&self, value: Option<CacheEntry<V>>, duration: Duration) -> Option<CacheEntry<V>> {
        if let Some(entry) = value {
            if self.is_expired(&entry) {
                self.telemetry.record_expired(self.name, duration, self.fallback);
                None
            } else {
                self.telemetry.record_hit(self.name, duration, self.fallback);
                Some(entry)
            }
        } else {
            self.telemetry.record_miss(self.name, duration, self.fallback);
            None
        }
    }
}

impl<K, V, CT> CacheTier<K, V> for CacheWrapper<K, V, CT>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
    CT: CacheTier<K, V> + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let watch = self.clock.stopwatch();
        match self.inner.get(key).await {
            Ok(value) => Ok(self.handle_get_result(value, watch.elapsed())),
            Err(e) => {
                self.telemetry.record_get_error(self.name, watch.elapsed(), self.fallback);
                Err(e)
            }
        }
    }

    async fn insert(&self, key: K, mut entry: CacheEntry<V>) -> Result<InsertOutcome, Error> {
        entry.ensure_cached_at(self.clock.system_time());
        if !self.policy.should_insert(&entry) {
            self.telemetry.record_insert_rejected(self.name, self.fallback);
            return Ok(InsertOutcome::Rejected);
        }

        let watch = self.clock.stopwatch();
        let result = self.inner.insert(key, entry).await;
        match &result {
            Ok(InsertOutcome::Accepted) => self.telemetry.record_inserted(self.name, watch.elapsed(), self.fallback),
            Ok(InsertOutcome::Rejected) => {
                self.telemetry
                    .record_insert_rejected_with_duration(self.name, watch.elapsed(), self.fallback);
            }
            Err(_) => self.telemetry.record_insert_error(self.name, watch.elapsed(), self.fallback),
        }
        result
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let watch = self.clock.stopwatch();
        let result = self.inner.invalidate(key).await;
        match &result {
            Ok(()) => self.telemetry.record_invalidated(self.name, watch.elapsed(), self.fallback),
            Err(_) => self.telemetry.record_invalidate_error(self.name, watch.elapsed(), self.fallback),
        }
        result
    }

    async fn clear(&self) -> Result<(), Error> {
        let watch = self.clock.stopwatch();
        let result = self.inner.clear().await;
        match &result {
            Ok(()) => self.telemetry.record_cleared(self.name, watch.elapsed(), self.fallback),
            Err(_) => self.telemetry.record_clear_error(self.name, watch.elapsed(), self.fallback),
        }
        result
    }

    async fn len(&self) -> Result<u64, SizeError> {
        self.inner.len().await
    }
}
