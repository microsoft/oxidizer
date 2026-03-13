// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier fallback cache implementation.
//!
//! This module provides fallback cache tiers that check a primary cache first,
//! then query a fallback tier on miss with configurable promotion policies.

use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;

use cachet_tier::{CacheEntry, CacheTier};
use futures::join;
use tick::Clock;

use crate::Error;
use crate::cache::CacheName;
use crate::refresh::TimeToRefresh;
use crate::telemetry::ext::ClockExt;
use crate::telemetry::{CacheActivity, CacheOperation, CacheTelemetry};

/// Type alias for promotion predicate functions.
type PromotionPredicate<V> = Arc<dyn Fn(&CacheEntry<V>) -> bool + Send + Sync>;

/// Policy for promoting values from fallback to primary cache.
///
/// When a cache miss occurs in the primary tier and a value is found in the
/// fallback tier, the promotion policy determines whether to copy that value
/// back to the primary tier for faster future access.
///
/// # Examples
///
/// ```
/// use cachet::FallbackPromotionPolicy;
///
/// // Always promote (default)
/// let policy = FallbackPromotionPolicy::<String>::always();
///
/// // Never promote
/// let policy = FallbackPromotionPolicy::<String>::never();
///
/// // Promote based on a condition
/// let policy = FallbackPromotionPolicy::<String>::when(|entry| entry.value().len() >= 5);
/// ```
#[derive(Debug, Default)]
pub struct FallbackPromotionPolicy<V>(PolicyType<V>);

#[derive(Default)]
enum PolicyType<V> {
    /// Always promote values to primary cache.
    #[default]
    Always,
    /// Never promote values to primary cache.
    Never,
    /// Promote based on a boxed predicate that can capture state.
    ///
    /// Use this when you need to capture external state in the predicate.
    /// Has slight overhead from dynamic dispatch.
    When(PromotionPredicate<V>),
}

impl<V> std::fmt::Debug for PolicyType<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Always => write!(f, "Always"),
            Self::Never => write!(f, "Never"),
            Self::When(_) => write!(f, "WhenBoxed(<closure>)"),
        }
    }
}

impl<V> FallbackPromotionPolicy<V> {
    /// Creates a policy that always promotes values to the primary cache.
    ///
    /// This is the default behavior and maximizes cache hit rates at the cost
    /// of additional writes to the primary tier.
    #[must_use]
    pub fn always() -> Self {
        Self(PolicyType::Always)
    }

    /// Creates a policy that never promotes values to the primary cache.
    ///
    /// Use this when the fallback tier is already fast enough and you want
    /// to avoid write overhead to the primary tier.
    #[must_use]
    pub fn never() -> Self {
        Self(PolicyType::Never)
    }

    /// Creates a policy using a predicate closure.
    ///
    /// The closure can capture external state if needed.
    ///
    /// ```no_run
    /// use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
    /// use tick::Clock;
    ///
    /// let min_len = 3;
    /// let clock = Clock::new_tokio();
    /// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .fallback(l2)
    ///     .promotion_policy(FallbackPromotionPolicy::when(
    ///         move |entry: &CacheEntry<String>| entry.value().len() >= min_len,
    ///     ))
    ///     .build();
    /// ```
    pub fn when<F>(predicate: F) -> Self
    where
        F: Fn(&CacheEntry<V>) -> bool + Send + Sync + 'static,
    {
        Self(PolicyType::When(Arc::new(predicate)))
    }

    /// Returns true if the response should be promoted to primary.
    #[inline]
    pub(crate) fn should_promote(&self, response: &CacheEntry<V>) -> bool {
        match &self.0 {
            PolicyType::Always => true,
            PolicyType::Never => false,
            PolicyType::When(pred) => pred(response),
        }
    }
}

pub(crate) struct FallbackCacheInner<K, V, P, F> {
    pub(crate) name: CacheName,
    pub(crate) primary: P,
    pub(crate) fallback: F,
    pub(crate) policy: FallbackPromotionPolicy<V>,
    pub(crate) clock: Clock,
    pub(crate) refresh: Option<TimeToRefresh<K>>,
    pub(crate) telemetry: CacheTelemetry,
    _phantom: PhantomData<K>,
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
/// On a primary cache miss, the fallback tier is queried. Based on the promotion
/// policy, successful fallback hits may be promoted back to the primary tier.
///
/// Construct this via `Cache::builder().fallback()` rather than directly.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use cachet::{Cache, FallbackPromotionPolicy};
/// use tick::Clock;
///
/// let clock = Clock::new_tokio();
/// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
///
/// let cache = Cache::builder::<String, String>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .fallback(l2)
///     .promotion_policy(FallbackPromotionPolicy::always())
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
        policy: FallbackPromotionPolicy<V>,
        clock: Clock,
        refresh: Option<TimeToRefresh<K>>,
        telemetry: CacheTelemetry,
    ) -> Self {
        Self {
            inner: Arc::new(FallbackCacheInner {
                name,
                primary,
                fallback,
                policy,
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
    /// Handles the fallback path when primary cache misses.
    /// This is a separate method so we can box just this path, keeping hits fast.
    async fn get_from_fallback(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let timed = self.inner.clock.timed_async(self.inner.fallback.get(key)).await;
        self.inner
            .telemetry
            .record(self.inner.name, CacheOperation::Get, CacheActivity::Fallback, timed.duration);

        // Propagate any error from fallback
        let fallback_value = timed.result?;

        if let Some(ref v) = fallback_value
            && self.inner.policy.should_promote(v)
        {
            let timed_insert = self.inner.clock.timed_async(self.inner.primary.insert(key, v.clone())).await;
            self.inner.telemetry.record(
                self.inner.name,
                CacheOperation::Insert,
                CacheActivity::FallbackPromotion,
                timed_insert.duration,
            );
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
        // Primary lookup is not boxed to avoid allocation on the hot path (hits).
        // The fallback path is boxed to bound future size for deeply nested caches.
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

        // Fallback lookup - unboxed for benchmarking
        self.get_from_fallback(key).await
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(
            self.inner.primary.insert(key, entry.clone()),
            self.inner.fallback.insert(key, entry)
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

    fn len(&self) -> Option<u64> {
        // Return length of primary cache if available
        self.inner.primary.len()
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
    use crate::telemetry::TelemetryConfig;
    use crate::wrapper::CacheWrapper;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    fn make_primary() -> CacheWrapper<String, i32, MockCache<String, i32>> {
        let clock = Clock::new_frozen();
        let telemetry = TelemetryConfig::new().build();
        CacheWrapper::new("primary", MockCache::new(), clock, None, telemetry)
    }

    fn make_fallback_cache(
        policy: FallbackPromotionPolicy<i32>,
    ) -> FallbackCache<String, i32, CacheWrapper<String, i32, MockCache<String, i32>>, MockCache<String, i32>> {
        let clock = Clock::new_frozen();
        let primary = make_primary();
        let fallback_mock = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        FallbackCache::new("fallback", primary, fallback_mock, policy, clock, None, telemetry)
    }

    /// Tests that promotion from fallback to primary works correctly.
    /// This test accesses internal state to verify promotion behavior.
    #[test]
    fn fallback_cachet_promotes_from_fallback_to_primary() {
        block_on(async {
            let clock = Clock::new_frozen();

            let primary_storage = MockCache::<String, i32>::new();
            let primary_check = primary_storage.clone();
            let fallback_storage = MockCache::<String, i32>::new();

            fallback_storage
                .insert(&"key".to_string(), CacheEntry::new(42))
                .await
                .expect("insert failed");

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .fallback(fallback)
                .promotion_policy(FallbackPromotionPolicy::always())
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
        });
    }

    /// Tests that Never promotion policy prevents promotion to primary.
    /// This test accesses internal state to verify no promotion occurs.
    #[test]
    fn fallback_cachet_never_policy_does_not_promote() {
        block_on(async {
            let clock = Clock::new_frozen();

            let primary_storage = MockCache::<String, i32>::new();
            let primary_check = primary_storage.clone();
            let fallback_storage = MockCache::<String, i32>::new();

            fallback_storage
                .insert(&"key".to_string(), CacheEntry::new(42))
                .await
                .expect("insert failed");

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .fallback(fallback)
                .promotion_policy(FallbackPromotionPolicy::never())
                .build();

            // Get should find in fallback but NOT promote
            let result = cache.get(&"key".to_string()).await.expect("get failed");
            assert!(result.is_some());
            assert_eq!(*result.unwrap().value(), 42);

            // Primary should still be empty (no promotion)
            let primary_result = primary_check.get(&"key".to_string()).await.expect("get failed");
            assert!(primary_result.is_none());
        });
    }

    /// Tests that `FallbackCacheInner` Debug output is correct.
    #[test]
    fn fallback_cachet_inner_debug() {
        let cache = make_fallback_cache(FallbackPromotionPolicy::always());

        let debug_str = format!("{:?}", cache);
        assert_eq!(debug_str, "FallbackCache { inner: FallbackCacheInner { name: \"fallback\", .. } }");
    }

    /// Tests that conditional promotion policy only promotes matching entries.
    /// This test accesses internal state to verify selective promotion.
    #[test]
    fn fallback_cachet_when_policy_conditional_promotion() {
        block_on(async {
            fn is_positive(entry: &CacheEntry<i32>) -> bool {
                *entry.value() > 0
            }

            let clock = Clock::new_frozen();

            let primary_storage = MockCache::<String, i32>::new();
            let primary_check = primary_storage.clone();
            let fallback_storage = MockCache::<String, i32>::new();

            fallback_storage
                .insert(&"positive".to_string(), CacheEntry::new(42))
                .await
                .expect("insert failed");
            fallback_storage
                .insert(&"negative".to_string(), CacheEntry::new(-10))
                .await
                .expect("insert failed");

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .fallback(fallback)
                .promotion_policy(FallbackPromotionPolicy::when(is_positive))
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
        });
    }

    #[test]
    fn policy_type_debug_formatting() {
        let always = FallbackPromotionPolicy::<i32>::always();
        let never = FallbackPromotionPolicy::<i32>::never();
        let when = FallbackPromotionPolicy::<i32>::when(|_| true);

        let always_str = format!("{always:?}");
        let never_str = format!("{never:?}");
        let when_str = format!("{when:?}");

        assert!(always_str.contains("Always"), "got: {always_str}");
        assert!(never_str.contains("Never"), "got: {never_str}");
        assert!(when_str.contains("WhenBoxed"), "got: {when_str}");
    }

    #[test]
    fn promotion_policy_always() {
        let policy = FallbackPromotionPolicy::<i32>::always();
        let entry = CacheEntry::new(42);
        assert!(policy.should_promote(&entry));
    }

    #[test]
    fn promotion_policy_never() {
        let policy = FallbackPromotionPolicy::<i32>::never();
        let entry = CacheEntry::new(42);
        assert!(!policy.should_promote(&entry));
    }

    #[test]
    fn promotion_policy_when() {
        let policy = FallbackPromotionPolicy::<i32>::when(|e| *e.value() > 10);
        assert!(policy.should_promote(&CacheEntry::new(42)));
        assert!(!policy.should_promote(&CacheEntry::new(5)));
    }

    #[test]
    fn fallback_cache_new_constructs() {
        let cache = make_fallback_cache(FallbackPromotionPolicy::always());
        assert_eq!(cache.inner.name, "fallback");
    }

    #[test]
    fn fallback_get_miss_both() {
        block_on(async {
            let cache = make_fallback_cache(FallbackPromotionPolicy::always());
            let result = cache.get(&"key".to_string()).await.unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn fallback_insert_writes_both() {
        block_on(async {
            let cache = make_fallback_cache(FallbackPromotionPolicy::always());
            cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
            // Both tiers should have the value
            let entry = cache.get(&"key".to_string()).await.unwrap().unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn fallback_invalidate() {
        block_on(async {
            let cache = make_fallback_cache(FallbackPromotionPolicy::always());
            cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
            cache.invalidate(&"key".to_string()).await.unwrap();
            assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
        });
    }

    #[test]
    fn fallback_clear() {
        block_on(async {
            let cache = make_fallback_cache(FallbackPromotionPolicy::always());
            cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
            cache.clear().await.unwrap();
            assert!(cache.get(&"key".to_string()).await.unwrap().is_none());
        });
    }

    #[test]
    fn fallback_len() {
        block_on(async {
            let cache = make_fallback_cache(FallbackPromotionPolicy::always());
            assert_eq!(cache.len(), Some(0));
            cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
            assert_eq!(cache.len(), Some(1));
        });
    }

    /// Exercises the background-refresh-on-get path: when a primary hit has a
    /// stale `cached_at`, `FallbackCache::get` should trigger `do_refresh`.
    #[tokio::test]
    async fn fallback_get_triggers_background_refresh() {
        let clock = Clock::new_frozen();
        let primary_mock = MockCache::<String, i32>::new();

        // Insert an entry with an old cached_at so should_refresh returns true
        let old_time = clock.system_time() - Duration::from_secs(120);
        let entry = CacheEntry::expires_at(42, Duration::from_secs(300), old_time);
        primary_mock.insert(&"key".to_string(), entry).await.unwrap();

        let fallback_mock = MockCache::<String, i32>::new();
        let telemetry = TelemetryConfig::new().build();
        let refresh = crate::refresh::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio());

        let primary = CacheWrapper::new("primary", primary_mock, clock.clone(), None, telemetry.clone());
        let fc = FallbackCache::new(
            "test",
            primary,
            fallback_mock,
            FallbackPromotionPolicy::always(),
            clock,
            Some(refresh),
            telemetry,
        );

        // Primary hit with stale cached_at should trigger background refresh
        let result = fc.get(&"key".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);

        // Give the spawned refresh task time to run
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[test]
    fn do_refresh_without_time_to_refresh_is_noop() {
        let cache = make_fallback_cache(FallbackPromotionPolicy::always());

        // Calling do_refresh should silently return (exercise the else branch)
        cache.do_refresh(&"key".to_string());
    }
}
