// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier fallback cache implementation.
//!
//! This module provides fallback cache tiers that check a primary cache first,
//! then query a fallback tier on miss with configurable promotion policies.

use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;

use cachet_tier::{CacheEntry, CacheTier, InsertOutcome, SizeError};
use futures::join;
use tick::Clock;

use crate::Error;
use crate::cache::CacheName;
use crate::refresh::TimeToRefresh;
use crate::telemetry::CacheTelemetry;

#[derive(derive_more::Debug)]
pub(crate) struct FallbackCacheInner<K, V, P, F> {
    pub(crate) name: CacheName,
    // P and F need not implement Debug.
    #[debug(skip)]
    pub(crate) primary: P,
    #[debug(skip)]
    pub(crate) fallback: F,
    pub(crate) clock: Clock,
    pub(crate) refresh: Option<TimeToRefresh<K>>,
    pub(crate) telemetry: CacheTelemetry,
    _phantom: PhantomData<(K, V)>,
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
pub(crate) struct FallbackCache<K, V, P, F> {
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
        let fallback_value = self.inner.fallback.get(key).await?;

        if let Some(ref v) = fallback_value {
            // Insert errors are intentionally swallowed - a failed promotion should not
            // fail the overall get. CacheWrapper records the insert outcome, while this
            // match records the corresponding promotion outcome.
            match self.inner.primary.insert(key.clone(), v.clone()).await {
                Ok(InsertOutcome::Accepted) => self.inner.telemetry.record_promotion_accepted(self.inner.name),
                Ok(InsertOutcome::Rejected) => self.inner.telemetry.record_promotion_rejected(self.inner.name),
                Err(_) => self.inner.telemetry.record_promotion_failed(self.inner.name),
            }
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

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<InsertOutcome, Error> {
        let (primary_result, fallback_result) = join!(
            self.inner.primary.insert(key.clone(), entry.clone()),
            self.inner.fallback.insert(key, entry)
        );
        let primary_outcome = primary_result?;
        let fallback_outcome = fallback_result?;

        if primary_outcome == InsertOutcome::Accepted || fallback_outcome == InsertOutcome::Accepted {
            Ok(InsertOutcome::Accepted)
        } else {
            Ok(InsertOutcome::Rejected)
        }
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
// Cache hides the fallback's Debug representation and construction state.
// Public reads call do_refresh only when refresh is configured.
#[cfg(test)]
mod tests {
    use cachet_tier::MockCache;

    use super::*;
    use crate::InsertPolicy;
    use crate::wrapper::CacheWrapper;
    type TestPrimary = CacheWrapper<String, i32, MockCache<String, i32>>;
    type TestFallbackCache = FallbackCache<String, i32, TestPrimary, MockCache<String, i32>>;
    /// Tests that `FallbackCacheInner` Debug output is correct.
    #[test]
    #[cfg_attr(miri, ignore = "Debug snapshots run natively and under cargo careful")]
    fn fallback_cachet_inner_debug() {
        let mut cache = make_fallback_cache();
        Arc::get_mut(&mut cache.inner).unwrap().refresh = Some(TimeToRefresh::new(
            std::time::Duration::from_mins(1),
            anyspawn::Spawner::new_tokio(None),
        ));
        insta::assert_debug_snapshot!(cache);
    }

    fn make_primary() -> TestPrimary {
        let clock = Clock::new_frozen();
        let telemetry = CacheTelemetry::new();
        CacheWrapper::new("primary", MockCache::new(), clock, None, telemetry, InsertPolicy::default(), false)
    }

    fn make_fallback_cache() -> TestFallbackCache {
        let clock = Clock::new_frozen();
        let primary = make_primary();
        let fallback_mock = MockCache::<String, i32>::new();
        let telemetry = CacheTelemetry::new();
        FallbackCache::new("fallback", primary, fallback_mock, clock, None, telemetry)
    }

    #[test]
    fn do_refresh_without_time_to_refresh_is_noop() {
        let cache = make_fallback_cache();

        // Calling do_refresh should silently return (exercise the else branch)
        cache.do_refresh(&"key".to_string());
    }
}
