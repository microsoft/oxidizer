// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier fallback cache implementation.
//!
//! This module provides fallback cache tiers that check a primary cache first,
//! then query a fallback tier on miss with configurable promotion policies.

use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;

use cachet_tier::{CacheEntry, CacheTier, SizeError};
use futures::join;
use tick::Clock;

use crate::Error;
use crate::cache::CacheName;
use crate::refresh::TimeToRefresh;
use crate::telemetry::ext::ClockExt;
use crate::telemetry::{CacheActivity, CacheOperation, CacheTelemetry};

pub(crate) struct FallbackCacheInner<K, V, P, F> {
    pub(crate) name: CacheName,
    pub(crate) primary: P,
    pub(crate) fallback: F,
    pub(crate) clock: Clock,
    pub(crate) refresh: Option<TimeToRefresh<K>>,
    pub(crate) telemetry: CacheTelemetry,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, P, F> std::fmt::Debug for FallbackCacheInner<K, V, P, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackCacheInner")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A two-tier cache that checks a primary tier, then falls back to a secondary tier.
///
/// On a primary cache miss, the fallback tier is queried. Successful fallback hits
/// may be promoted back to the primary tier.
///
/// Construct this via `Cache::builder().fallback()` rather than directly.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use cachet::Cache;
/// use tick::Clock;
///
/// let clock = Clock::new_tokio();
/// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
///
/// let cache = Cache::builder::<String, String>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .fallback(l2)
///     .build();
/// ```
#[derive(Debug)]
pub struct FallbackCache<K, V, P, F> {
    pub(crate) inner: Arc<FallbackCacheInner<K, V, P, F>>,
}

impl<K, V, P, F> FallbackCache<K, V, P, F> {
    /// Creates a new fallback cache with a primary and type-erased fallback tier.
    pub(crate) fn new(
        name: CacheName,
        primary: P,
        fallback: F,
        clock: Clock,
        refresh: Option<TimeToRefresh<K>>,
        telemetry: CacheTelemetry,
    ) -> Self {
        Self {
            inner: Arc::new(FallbackCacheInner {
                name,
                primary,
                fallback,
                clock,
                refresh,
                telemetry,
                _phantom: PhantomData,
            }),
        }
    }
}

impl<K, V, P, F> FallbackCache<K, V, P, F>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    P: CacheTier<K, V> + Send + Sync + 'static,
    F: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Handles the fallback path when the primary cache misses.
    ///
    /// Separated from [`get`](Self::get) to keep the hot path (primary hits) small.
    async fn get_from_fallback(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let timed = self.inner.clock.timed_async(self.inner.fallback.get(key)).await;
        self.inner
            .telemetry
            .record(self.inner.name, CacheOperation::Get, CacheActivity::Fallback, timed.duration);

        // Propagate any error from fallback
        let fallback_value = timed.result?;

        if let Some(ref v) = fallback_value {
            // Insert errors are intentionally swallowed - a failed promotion should not
            // fail the overall get. The CacheWrapper around the primary tier already
            // records telemetry for the insert (Inserted, Rejected, or Error).
            let _ = self.inner.primary.insert(key.clone(), v.clone()).await;
        }

        Ok(fallback_value)
    }
}

impl<K, V, P, F> CacheTier<K, V> for FallbackCache<K, V, P, F>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    P: CacheTier<K, V> + Send + Sync + 'static,
    F: CacheTier<K, V> + Send + Sync + 'static,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        // The fallback path is in a separate method to keep the primary hit path small.
        // Primary errors are already logged by the inner CacheWrapper.
        if let Ok(Some(value)) = self.inner.primary.get(key).await {
            // Check if background refresh is needed
            if let Some(refresh) = &self.inner.refresh
                && let Some(cached_at) = value.cached_at()
                && refresh.should_refresh(cached_at, self.inner.clock.system_time())
            {
                self.do_refresh(key);
            }
            return Ok(Some(value));
        }

        self.get_from_fallback(key).await
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(
            self.inner.primary.insert(key.clone(), entry.clone()),
            self.inner.fallback.insert(key.clone(), entry)
        );
        primary_result?;
        fallback_result
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(self.inner.primary.invalidate(key), self.inner.fallback.invalidate(key));
        primary_result?;
        fallback_result
    }

    async fn clear(&self) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(self.inner.primary.clear(), self.inner.fallback.clear());
        primary_result?;
        fallback_result
    }

    async fn len(&self) -> Result<u64, SizeError> {
        // Return length of primary cache if available
        self.inner.primary.len().await
    }
}

// NOTE: Service implementation is only provided for the top-level Cache type,
// not for internal types like FallbackCache. This keeps the service boundary
// clean and focused on the user-facing API.

/// Unit tests for internal fallback cache implementation details.
///
/// Public API tests are in `tests/fallback.rs`.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cachet_tier::MockCache;

    use super::*;
    use crate::Cache;
    use crate::InsertPolicy;
    use crate::telemetry::TelemetryConfig;
    use crate::wrapper::CacheWrapper;

    type TestPrimary = CacheWrapper<String, i32, MockCache<String, i32>>;
    type TestFallbackCache = FallbackCache<String, i32, TestPrimary, MockCache<String, i32>>;

    fn make_primary() -> TestPrimary {
        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        CacheWrapper::new("primary", MockCache::new(), clock, None, telemetry, InsertPolicy::default())
    }

    fn make_fallback_cache() -> TestFallbackCache {
        let clock = Clock::new_frozen();
        let primary = make_primary();
        let fallback_mock = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        FallbackCache::new("fallback", primary, fallback_mock, clock, None, telemetry)
    }

    /// Tests that promotion from fallback to primary works correctly.
    /// This test accesses internal state to verify promotion behavior.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_cachet_promotes_from_fallback_to_primary() {
        let clock = Clock::new_frozen();

        let primary_storage = MockCache::<String, i32>::new();
        let primary_check = primary_storage.clone();
        let fallback_storage = MockCache::<String, i32>::new();

        fallback_storage
            .insert("key".to_string(), CacheEntry::new(42))
            .await
            .expect("insert failed");

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .insert_policy(InsertPolicy::always())
            .fallback(fallback)
            .build();

        // Primary should be empty initially
        let primary_result = primary_check.get(&"key".to_string()).await.expect("get failed");
        assert!(primary_result.is_none());

        // Get should find in fallback and promote to primary
        let result = cache.get(&"key".to_string()).await.expect("get failed");
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Now primary should have the value (promoted from fallback)
        let primary_result = primary_check.get(&"key".to_string()).await.expect("get failed");
        assert!(primary_result.is_some());
    }

    /// Tests that Never promotion policy prevents promotion to primary.
    /// This test accesses internal state to verify no promotion occurs.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_cachet_never_policy_does_not_promote() {
        let clock = Clock::new_frozen();

        let primary_storage = MockCache::<String, i32>::new();
        let primary_check = primary_storage.clone();
        let fallback_storage = MockCache::<String, i32>::new();

        fallback_storage
            .insert("key".to_string(), CacheEntry::new(42))
            .await
            .expect("insert failed");

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .insert_policy(InsertPolicy::never())
            .fallback(fallback)
            .build();

        // Get should find in fallback but NOT promote
        let result = cache.get(&"key".to_string()).await.expect("get failed");
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Primary should still be empty (no promotion)
        let primary_result = primary_check.get(&"key".to_string()).await.expect("get failed");
        assert!(primary_result.is_none());
    }

    /// Tests that `FallbackCacheInner` Debug output is correct.
    #[test]
    fn fallback_cachet_inner_debug() {
        let cache = make_fallback_cache();

        let debug_str = format!("{cache:?}");
        assert_eq!(debug_str, "FallbackCache { inner: FallbackCacheInner { name: \"fallback\", .. } }");
    }

    /// Tests that conditional promotion policy only promotes matching entries.
    /// This test accesses internal state to verify selective promotion.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_cachet_when_policy_conditional_promotion() {
        fn is_positive(entry: &CacheEntry<i32>) -> bool {
            *entry.value() > 0
        }

        let clock = Clock::new_frozen();

        let primary_storage = MockCache::<String, i32>::new();
        let primary_check = primary_storage.clone();
        let fallback_storage = MockCache::<String, i32>::new();

        fallback_storage
            .insert("positive".to_string(), CacheEntry::new(42))
            .await
            .expect("insert failed");
        fallback_storage
            .insert("negative".to_string(), CacheEntry::new(-10))
            .await
            .expect("insert failed");

        let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

        let cache = Cache::builder::<String, i32>(clock)
            .storage(primary_storage)
            .insert_policy(InsertPolicy::when(is_positive))
            .fallback(fallback)
            .build();

        // Get positive value - should be promoted
        let result = cache.get(&"positive".to_string()).await.expect("get failed");
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Get negative value - should NOT be promoted
        let result = cache.get(&"negative".to_string()).await.expect("get failed");
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), -10);

        // Check primary has positive but not negative
        let positive = primary_check.get(&"positive".to_string()).await.expect("get failed");
        assert!(positive.is_some());
        let negative = primary_check.get(&"negative".to_string()).await.expect("get failed");
        assert!(negative.is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn policy_type_debug_formatting() {
        let always = InsertPolicy::<i32>::always();
        let never = InsertPolicy::<i32>::never();
        let when = InsertPolicy::<i32>::when(|_| true);

        let always_str = format!("{always:?}");
        let never_str = format!("{never:?}");
        let when_str = format!("{when:?}");

        assert!(always_str.contains("Always"), "got: {always_str}");
        assert!(never_str.contains("Never"), "got: {never_str}");
        assert!(when_str.contains("WhenBoxed"), "got: {when_str}");
    }

    #[test]
    fn insert_policy_always() {
        let policy = InsertPolicy::<i32>::always();
        let entry = CacheEntry::new(42);
        assert!(policy.should_insert(&entry));
    }

    #[test]
    fn insert_policy_never() {
        let policy = InsertPolicy::<i32>::never();
        let entry = CacheEntry::new(42);
        assert!(!policy.should_insert(&entry));
    }

    #[test]
    fn insert_policy_when() {
        let policy = InsertPolicy::<i32>::when(|e| *e.value() > 10);
        assert!(policy.should_insert(&CacheEntry::new(42)));
        assert!(!policy.should_insert(&CacheEntry::new(5)));
    }

    #[test]
    fn fallback_cache_new_constructs() {
        let cache = make_fallback_cache();
        assert_eq!(cache.inner.name, "fallback");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_get_miss_both() {
        let cache = make_fallback_cache();
        let result = cache.get(&"key".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_insert_writes_both() {
        let cache = make_fallback_cache();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        // Both tiers should have the value
        let entry = cache.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_invalidate() {
        let cache = make_fallback_cache();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        cache.invalidate(&"key".to_string()).await.unwrap();
        assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_clear() {
        let cache = make_fallback_cache();
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        cache.clear().await.unwrap();
        assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_len() {
        let cache = make_fallback_cache();
        assert_eq!(cache.len().await.expect("len should return Ok"), 0);
        cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        assert_eq!(cache.len().await.expect("len should return Ok"), 1);
    }

    /// Exercises the background-refresh-on-get path: when a primary hit has a
    /// stale `cached_at`, `FallbackCache::get` should trigger `do_refresh`.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_get_triggers_background_refresh() {
        let clock = Clock::new_frozen();
        let primary_mock = MockCache::<String, i32>::new();

        // Insert an entry with an old cached_at so should_refresh returns true
        let old_time = clock.system_time() - Duration::from_secs(120);
        let entry = CacheEntry::expires_at(42, Duration::from_secs(300), old_time);
        primary_mock.insert("key".to_string(), entry).await.unwrap();

        let fallback_mock = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let refresh = crate::refresh::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio());

        let primary = CacheWrapper::new(
            "primary",
            primary_mock,
            clock.clone(),
            None,
            telemetry.clone(),
            InsertPolicy::default(),
        );
        let fc = FallbackCache::new("test", primary, fallback_mock, clock, Some(refresh), telemetry);

        // Primary hit with stale cached_at should trigger background refresh
        let result = fc.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Give the spawned refresh task time to run
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[test]
    fn do_refresh_without_time_to_refresh_is_noop() {
        let cache = make_fallback_cache();

        // Calling do_refresh should silently return (exercise the else branch)
        cache.do_refresh(&"key".to_string());
    }
}
