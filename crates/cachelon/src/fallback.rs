// Copyright (c) Microsoft Corporation.

//! Multi-tier fallback cache implementation.
//!
//! This module provides fallback cache tiers that check a primary cache first,
//! then query a fallback tier on miss with configurable promotion policies.

use std::{hash::Hash, marker::PhantomData, sync::Arc, time::Duration};

use futures::join;
use tick::Clock;

#[cfg(any(feature = "tokio", test))]
use crate::refresh::TimeToRefresh;
#[cfg(feature = "telemetry")]
use crate::telemetry::{CacheEvent, CacheOperation, CacheTelemetry};
use crate::{
    Error,
    cache::CacheName,
    telemetry::ext::{CacheTelemetryExt, ClockExt},
};
use cachelon_tier::{CacheEntry, CacheTier};

/// Policy for promoting values from fallback to primary cache.
///
/// When a cache miss occurs in the primary tier and a value is found in the
/// fallback tier, the promotion policy determines whether to copy that value
/// back to the primary tier for faster future access.
///
/// # Examples
///
/// ```
/// use cachelon::FallbackPromotionPolicy;
///
/// // Always promote (default)
/// let policy = FallbackPromotionPolicy::<String>::always();
///
/// // Never promote
/// let policy = FallbackPromotionPolicy::<String>::never();
/// ```
#[derive(Default)]
#[expect(clippy::type_complexity, reason = "boxed closure type is necessary")]
pub enum FallbackPromotionPolicy<V> {
    /// Always promote values to primary cache.
    #[default]
    Always,
    /// Never promote values to primary cache.
    Never,
    /// Promote based on a function pointer predicate.
    ///
    /// Use this for simple predicates without captured state - it has zero
    /// allocation overhead and is slightly faster than `WhenBoxed`.
    When(fn(&CacheEntry<V>) -> bool),
    /// Promote based on a boxed predicate that can capture state.
    ///
    /// Use this when you need to capture external state in the predicate.
    /// Has slight overhead from dynamic dispatch.
    WhenBoxed(Arc<dyn Fn(&CacheEntry<V>) -> bool + Send + Sync>),
}

impl<V> std::fmt::Debug for FallbackPromotionPolicy<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Always => write!(f, "Always"),
            Self::Never => write!(f, "Never"),
            Self::When(_) => write!(f, "When(<fn>)"),
            Self::WhenBoxed(_) => write!(f, "WhenBoxed(<closure>)"),
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
        Self::Always
    }

    /// Creates a policy that never promotes values to the primary cache.
    ///
    /// Use this when the fallback tier is already fast enough and you want
    /// to avoid write overhead to the primary tier.
    #[must_use]
    pub fn never() -> Self {
        Self::Never
    }

    /// Creates a policy using a function pointer predicate.
    ///
    /// This is the most efficient option when no captured state is needed.
    ///
    /// ```
    /// use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
    /// use tick::Clock;
    ///
    /// fn should_promote(entry: &CacheEntry<String>) -> bool {
    ///     !entry.value().is_empty()
    /// }
    ///
    /// let clock = Clock::new_frozen();
    /// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .with_fallback(l2)
    ///     .promotion_policy(FallbackPromotionPolicy::when(should_promote))
    ///     .build();
    /// ```
    pub fn when(predicate: fn(&CacheEntry<V>) -> bool) -> Self {
        Self::When(predicate)
    }

    /// Creates a policy using a closure that can capture state.
    ///
    /// Use this when you need to capture external variables in the predicate.
    ///
    /// ```
    /// use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
    /// use tick::Clock;
    ///
    /// let min_len = 3;
    /// let clock = Clock::new_frozen();
    /// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .with_fallback(l2)
    ///     .promotion_policy(FallbackPromotionPolicy::when_boxed(
    ///         move |entry: &CacheEntry<String>| entry.value().len() >= min_len
    ///     ))
    ///     .build();
    /// ```
    pub fn when_boxed<F>(predicate: F) -> Self
    where
        F: Fn(&CacheEntry<V>) -> bool + Send + Sync + 'static,
    {
        Self::WhenBoxed(Arc::new(predicate))
    }

    /// Returns true if the response should be promoted to primary.
    #[inline]
    pub(crate) fn should_promote(&self, response: &CacheEntry<V>) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::When(pred) => pred(response),
            Self::WhenBoxed(pred) => pred(response),
        }
    }
}

pub(crate) struct FallbackCacheInner<K, V, P, F> {
    pub(crate) name: CacheName,
    pub(crate) primary: P,
    pub(crate) fallback: F,
    pub(crate) policy: FallbackPromotionPolicy<V>,
    pub(crate) clock: Clock,
    #[cfg(any(feature = "tokio", test))]
    pub(crate) refresh: Option<TimeToRefresh<K>>,
    #[cfg(feature = "telemetry")]
    pub(crate) telemetry: Option<CacheTelemetry>,
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
/// Construct this via `Cache::builder().with_fallback()` rather than directly.
///
/// # Examples
///
/// ```
/// use cachelon::{Cache, FallbackPromotionPolicy};
/// use tick::Clock;
/// use std::time::Duration;
/// # futures::executor::block_on(async {
///
/// let clock = Clock::new_frozen();
/// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
///
/// let cache = Cache::builder::<String, String>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .with_fallback(l2)
///     .promotion_policy(FallbackPromotionPolicy::always())
///     .build();
/// # });
/// ```
#[derive(Debug)]
pub struct FallbackCache<K, V, P, F> {
    pub(crate) inner: Arc<FallbackCacheInner<K, V, P, F>>,
}

impl<K, V, P, F> FallbackCache<K, V, P, F> {
    /// Creates a new fallback cache with a primary and fallback tier.
    pub(crate) fn new(
        name: CacheName,
        primary: P,
        fallback: F,
        policy: FallbackPromotionPolicy<V>,
        clock: Clock,
        #[cfg(any(feature = "tokio", test))] refresh: Option<TimeToRefresh<K>>,
        #[cfg(feature = "telemetry")] telemetry: Option<CacheTelemetry>,
    ) -> Self {
        Self {
            inner: Arc::new(FallbackCacheInner {
                name,
                primary,
                fallback,
                policy,
                clock,
                #[cfg(any(feature = "tokio", test))]
                refresh,
                #[cfg(feature = "telemetry")]
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
    async fn handle_get(&self, key: &K, value: Option<CacheEntry<V>>) -> Option<CacheEntry<V>> {
        if let Some(value) = value {
            if let Some(refresh) = &self.inner.refresh
                && let Some(cached_at) = value.cached_at()
                && refresh.should_refresh(cached_at)
            {
                self.do_refresh(key);
            }

            Some(value)
        } else {
            let timed = self.inner.clock.timed_async(self.inner.fallback.get(key)).await;

            #[cfg(feature = "telemetry")]
            self.inner
                .telemetry
                .record(self.inner.name, CacheOperation::Get, CacheEvent::Fallback, timed.duration);

            if let Some(ref v) = timed.result
                && self.inner.policy.should_promote(v)
            {
                let timed_insert = self.inner.clock.timed_async(self.inner.primary.insert(key, v.clone())).await;

                #[cfg(feature = "telemetry")]
                self.inner.telemetry.record(
                    self.inner.name,
                    CacheOperation::Insert,
                    CacheEvent::FallbackPromotion,
                    timed_insert.duration,
                );
            }
            timed.result
        }
    }

    async fn handle_try_get(
        &self,
        key: &K,
        result: Result<Option<CacheEntry<V>>, Error>,
        duration: Duration,
    ) -> Result<Option<CacheEntry<V>>, Error> {
        match result {
            Ok(value) => Ok(self.handle_get(key, value).await),
            Err(e) => {
                #[cfg(feature = "telemetry")]
                self.inner
                    .telemetry
                    .record(self.inner.name, CacheOperation::Get, CacheEvent::Error, duration);
                Err(e)
            }
        }
    }
}

impl<K, V, P, F> CacheTier<K, V> for FallbackCache<K, V, P, F>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    P: CacheTier<K, V> + Send + Sync + 'static,
    F: CacheTier<K, V> + Send + Sync + 'static,
{
    async fn get(&self, key: &K) -> Option<CacheEntry<V>> {
        self.handle_get(key, self.inner.primary.get(key).await).await
    }

    async fn try_get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let timed = self.inner.clock.timed_async(self.inner.primary.try_get(key)).await;
        self.handle_try_get(key, timed.result, timed.duration).await
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) {
        join!(
            self.inner.primary.insert(key, entry.clone()),
            self.inner.fallback.insert(key, entry)
        );
    }

    async fn try_insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(
            self.inner.primary.try_insert(key, entry.clone()),
            self.inner.fallback.try_insert(key, entry)
        );
        primary_result?;
        fallback_result
    }

    async fn invalidate(&self, key: &K) {
        join!(self.inner.primary.invalidate(key), self.inner.fallback.invalidate(key));
    }

    async fn try_invalidate(&self, key: &K) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(self.inner.primary.try_invalidate(key), self.inner.fallback.try_invalidate(key));
        primary_result?;
        fallback_result
    }

    async fn clear(&self) {
        join!(self.inner.primary.clear(), self.inner.fallback.clear());
    }

    async fn try_clear(&self) -> Result<(), Error> {
        let (primary_result, fallback_result) = join!(self.inner.primary.try_clear(), self.inner.fallback.try_clear());
        primary_result?;
        fallback_result
    }

    fn len(&self) -> Option<u64> {
        // Return length of primary cache if available
        self.inner.primary.len()
    }

    fn is_empty(&self) -> Option<bool> {
        self.inner.primary.is_empty()
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
    use super::*;
    use crate::Cache;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    /// Tests that promotion from fallback to primary works correctly.
    /// This test accesses internal state to verify promotion behavior.
    #[test]
    fn fallback_cachelon_promotes_from_fallback_to_primary() {
        block_on(async {
            let clock = Clock::new_frozen();

            let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
            let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

            fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await;

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .with_fallback(fallback)
                .promotion_policy(FallbackPromotionPolicy::Always)
                .build();

            // Primary should be empty initially
            let primary_inner = cache.inner();
            assert!(primary_inner.inner.primary.get(&"key".to_string()).await.is_none());

            // Get should find in fallback and promote to primary
            let result = cache.get(&"key".to_string()).await;
            assert!(result.is_some());
            assert_eq!(*result.unwrap().value(), 42);

            // Now primary should have the value (promoted from fallback)
            let primary_result = primary_inner.inner.primary.get(&"key".to_string()).await;
            assert!(primary_result.is_some());
        });
    }

    /// Tests that Never promotion policy prevents promotion to primary.
    /// This test accesses internal state to verify no promotion occurs.
    #[test]
    fn fallback_cachelon_never_policy_does_not_promote() {
        block_on(async {
            let clock = Clock::new_frozen();

            let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
            let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

            fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await;

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .with_fallback(fallback)
                .promotion_policy(FallbackPromotionPolicy::Never)
                .build();

            // Get should find in fallback but NOT promote
            let result = cache.get(&"key".to_string()).await;
            assert!(result.is_some());
            assert_eq!(*result.unwrap().value(), 42);

            // Primary should still be empty (no promotion)
            let primary_inner = cache.inner();
            let primary_result = primary_inner.inner.primary.get(&"key".to_string()).await;
            assert!(primary_result.is_none());
        });
    }

    /// Tests that FallbackCacheInner Debug output is correct.
    #[test]
    fn fallback_cachelon_inner_debug() {
        let clock = Clock::new_frozen();

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock).memory().with_fallback(fallback).build();

        let inner = cache.inner();
        let debug_str = format!("{:?}", inner.inner);
        assert!(debug_str.contains("FallbackCacheInner"));
    }

    /// Tests that conditional promotion policy only promotes matching entries.
    /// This test accesses internal state to verify selective promotion.
    #[test]
    fn fallback_cachelon_when_policy_conditional_promotion() {
        block_on(async {
            let clock = Clock::new_frozen();

            let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
            let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

            fallback_storage.insert(&"positive".to_string(), CacheEntry::new(42)).await;
            fallback_storage.insert(&"negative".to_string(), CacheEntry::new(-10)).await;

            fn is_positive(entry: &CacheEntry<i32>) -> bool {
                *entry.value() > 0
            }

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .with_fallback(fallback)
                .promotion_policy(FallbackPromotionPolicy::when(is_positive))
                .build();

            // Get positive value - should be promoted
            let result = cache.get(&"positive".to_string()).await;
            assert!(result.is_some());
            assert_eq!(*result.unwrap().value(), 42);

            // Get negative value - should NOT be promoted
            let result = cache.get(&"negative".to_string()).await;
            assert!(result.is_some());
            assert_eq!(*result.unwrap().value(), -10);

            // Check primary has positive but not negative
            let primary_inner = cache.inner();
            assert!(primary_inner.inner.primary.get(&"positive".to_string()).await.is_some());
            assert!(primary_inner.inner.primary.get(&"negative".to_string()).await.is_none());
        });
    }

    /// Tests that try_get also triggers promotion from fallback.
    #[test]
    fn fallback_cachelon_try_get_with_promotion() {
        block_on(async {
            let clock = Clock::new_frozen();

            let primary_storage = cachelon_memory::InMemoryCache::<String, i32>::new();
            let fallback_storage = cachelon_memory::InMemoryCache::<String, i32>::new();

            fallback_storage.insert(&"key".to_string(), CacheEntry::new(42)).await;

            let fallback = Cache::builder::<String, i32>(clock.clone()).storage(fallback_storage);

            let cache = Cache::builder::<String, i32>(clock)
                .storage(primary_storage)
                .with_fallback(fallback)
                .build();

            // try_get should also trigger promotion
            let result = cache.try_get(&"key".to_string()).await;
            assert!(result.is_ok());
            assert!(result.unwrap().is_some());
        });
    }
}
