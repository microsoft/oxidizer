// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wrapper that adds telemetry and TTL to cache tiers.
//!
//! This module provides the `CacheWrapper` type that decorates any `CacheTier`
//! implementation with automatic telemetry recording and TTL expiration handling.

use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

use cachet_tier::CacheTier;
use tick::Clock;

use crate::cache::CacheName;
use crate::telemetry::ext::ClockExt;
use crate::telemetry::{CacheActivity, CacheOperation, CacheTelemetry};
use crate::{CacheEntry, Error};

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
pub struct CacheWrapper<K, V, CT> {
    pub(crate) name: CacheName,
    pub(crate) inner: CT,
    pub(crate) clock: Clock,
    pub(crate) ttl: Option<Duration>,
    pub(crate) telemetry: CacheTelemetry,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, CT> CacheWrapper<K, V, CT> {
    pub(crate) fn new(name: CacheName, inner: CT, clock: Clock, ttl: Option<Duration>, telemetry: CacheTelemetry) -> Self {
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
    pub fn inner(&self) -> &CT {
        &self.inner
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
                self.telemetry
                    .record(self.name, CacheOperation::Get, CacheActivity::Expired, duration);
                None
            } else {
                self.telemetry.record(self.name, CacheOperation::Get, CacheActivity::Hit, duration);
                Some(entry)
            }
        } else {
            self.telemetry.record(self.name, CacheOperation::Get, CacheActivity::Miss, duration);
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
        let timed = self.clock.timed_async(self.inner.get(key)).await;
        match timed.result {
            Ok(value) => Ok(self.handle_get_result(value, timed.duration)),
            Err(e) => {
                self.telemetry
                    .record(self.name, CacheOperation::Get, CacheActivity::Error, timed.duration);
                Err(e)
            }
        }
    }

    async fn insert(&self, key: K, mut entry: CacheEntry<V>) -> Result<(), Error> {
        entry.ensure_cached_at(self.clock.system_time());
        let timed = self.clock.timed_async(self.inner.insert(key, entry)).await;
        match &timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Insert, CacheActivity::Inserted, timed.duration);
                #[cfg(any(feature = "metrics", test))]
                if let Ok(Some(size)) = self.len().await {
                    self.telemetry.record_size(self.name, size);
                }
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Insert, CacheActivity::Error, timed.duration);
            }
        }
        timed.result
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let timed = self.clock.timed_async(self.inner.invalidate(key)).await;
        match &timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Invalidate, CacheActivity::Invalidated, timed.duration);
                #[cfg(any(feature = "metrics", test))]
                if let Ok(Some(size)) = self.len().await {
                    self.telemetry.record_size(self.name, size);
                }
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Invalidate, CacheActivity::Error, timed.duration);
            }
        }
        timed.result
    }

    async fn clear(&self) -> Result<(), Error> {
        let timed = self.clock.timed_async(self.inner.clear()).await;
        match &timed.result {
            Ok(()) => {
                self.telemetry
                    .record(self.name, CacheOperation::Clear, CacheActivity::Ok, timed.duration);
                #[cfg(any(feature = "metrics", test))]
                if let Ok(Some(size)) = self.len().await {
                    self.telemetry.record_size(self.name, size);
                }
            }
            Err(_) => {
                self.telemetry
                    .record(self.name, CacheOperation::Clear, CacheActivity::Error, timed.duration);
            }
        }
        timed.result
    }

    async fn len(&self) -> Result<Option<u64>, Error> {
        self.inner.len().await
    }
}

#[cfg(test)]
mod tests {
    use cachet_tier::MockCache;

    use super::*;
    use crate::telemetry::TelemetryConfig;

    #[test]
    fn wrapper_is_expired_with_no_ttl_returns_false() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);

        // Entry without TTL should not be expired
        let entry = CacheEntry::new(42);
        assert!(!wrapper.is_expired(&entry));
    }

    #[test]
    fn wrapper_is_expired_with_ttl_without_cached_at_returns_true() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, Some(Duration::from_secs(60)), telemetry);

        // Entry without cached_at should be expired if TTL is configured (treat as expired to be safe)
        let entry = CacheEntry::new(42);
        assert!(wrapper.is_expired(&entry));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_preserves_per_entry_ttl_over_tier_ttl() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let telemetry = TelemetryConfig::new().build();
        let tier_ttl = Duration::from_secs(60);
        let entry_ttl = Duration::from_secs(30);
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock.clone(), Some(tier_ttl), telemetry);

        let entry = CacheEntry::expires_at(42, entry_ttl, clock.system_time());

        wrapper.insert("key".to_string(), entry).await.unwrap();

        let stored = inner_check.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(stored.ttl(), Some(entry_ttl));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_without_tier_ttl_leaves_entry_ttl_unset() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);

        let entry = CacheEntry::new(42);
        wrapper.insert("key".to_string(), entry).await.unwrap();

        let stored = inner_check.get(&"key".to_string()).await.unwrap().unwrap();
        assert!(stored.ttl().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_with_tier_ttl_leaves_entry_ttl_unset() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let inner_check = inner.clone();
        let telemetry = TelemetryConfig::new().build();
        let tier_ttl = Duration::from_secs(60);
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, Some(tier_ttl), telemetry);

        let entry = CacheEntry::new(42);
        wrapper.insert("key".to_string(), entry).await.unwrap();

        let stored = inner_check.get(&"key".to_string()).await.unwrap().unwrap();
        assert!(stored.ttl().is_none());
    }

    #[test]
    fn wrapper_is_expired_when_system_time_goes_backward() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> =
            CacheWrapper::new("test", inner, clock.clone(), Some(Duration::from_secs(60)), telemetry);

        // Entry with cached_at in the future simulates clock going backward
        let entry = CacheEntry::expires_at(42, Duration::from_secs(60), clock.system_time() + Duration::from_secs(3600));
        assert!(wrapper.is_expired(&entry));
    }

    #[test]
    fn wrapper_is_not_expired_when_elapsed_equals_ttl() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let ttl = Duration::from_secs(60);
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock.clone(), Some(ttl), telemetry);

        // Entry cached exactly TTL ago → elapsed == ttl → should NOT be expired (uses >)
        let entry = CacheEntry::expires_at(42, ttl, clock.system_time() - ttl);
        assert!(!wrapper.is_expired(&entry));
    }

    #[test]
    fn mock_wrapper_new_and_accessors() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("mock_test", inner, clock, None, telemetry);
        assert_eq!(wrapper.name(), "mock_test");
        let _ = wrapper.inner();
    }

    #[test]
    fn mock_wrapper_handle_get_result_none() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        let result = wrapper.handle_get_result(None, Duration::from_secs(0));
        assert!(result.is_none());
    }

    #[test]
    fn mock_wrapper_handle_get_result_expired() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, Some(Duration::from_secs(60)), telemetry);
        // Entry without cached_at → considered expired
        let entry = CacheEntry::new(42);
        let result = wrapper.handle_get_result(Some(entry), Duration::from_secs(0));
        assert!(result.is_none());
    }

    #[test]
    fn mock_wrapper_handle_get_result_valid() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        let entry = CacheEntry::new(42);
        let result = wrapper.handle_get_result(Some(entry), Duration::from_secs(0));
        assert!(result.is_some());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_get_insert_invalidate_clear() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);

        // get miss
        assert!(wrapper.get(&"key".to_string()).await.unwrap().is_none());

        // insert + get hit
        wrapper.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        let entry = wrapper.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);

        // invalidate
        wrapper.invalidate(&"key".to_string()).await.unwrap();
        assert!(wrapper.get(&"key".to_string()).await.unwrap().is_none());

        // insert + clear
        wrapper.insert("a".to_string(), CacheEntry::new(1)).await.unwrap();
        wrapper.clear().await.unwrap();
        assert!(wrapper.get(&"a".to_string()).await.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_len() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        assert_eq!(wrapper.len().await.expect("len should return Ok"), Some(0));
        wrapper.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
        assert_eq!(wrapper.len().await.expect("len should return Ok"), Some(1));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_get_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Get(_)));
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        let result = wrapper.get(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_insert_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Insert { .. }));
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        let result = wrapper.insert("key".to_string(), CacheEntry::new(1)).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_invalidate_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Invalidate(_)));
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        let result = wrapper.invalidate(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mock_wrapper_clear_error() {
        let clock = Clock::new_frozen();
        let inner = MockCache::<String, i32>::new();
        inner.fail_when(|op| matches!(op, cachet_tier::CacheOp::Clear));
        let telemetry = TelemetryConfig::new().build();
        let wrapper: CacheWrapper<String, i32, _> = CacheWrapper::new("test", inner, clock, None, telemetry);
        let result = wrapper.clear().await;
        result.unwrap_err();
    }
}
