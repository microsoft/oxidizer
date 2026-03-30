// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

#[cfg(feature = "memory")]
use cachet_memory::InMemoryCache;
#[cfg(any(feature = "metrics", test))]
use opentelemetry::metrics::MeterProvider;
use tick::Clock;

use super::buildable::Buildable;
use super::fallback::FallbackBuilder;
use super::sealed::{CacheTierBuilder, Sealed};
use crate::fallback::FallbackPromotionPolicy;
use crate::telemetry::TelemetryConfig;
use crate::wrapper::CacheWrapper;
use crate::{Cache, CacheTier};

/// Builder for constructing a cache with a single tier.
///
/// Created by calling `Cache::builder()`. Allows configuring storage,
/// TTL, telemetry, and adding fallback tiers.
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
/// let cache = Cache::builder::<String, i32>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .build();
/// ```
#[derive(Debug)]
pub struct CacheBuilder<K, V, CT = ()> {
    pub(crate) name: Option<&'static str>,
    pub(crate) storage: CT,
    pub(crate) ttl: Option<Duration>,
    pub(crate) clock: Clock,
    pub(crate) telemetry: TelemetryConfig,
    pub(crate) stampede_protection: bool,
    pub(crate) _phantom: PhantomData<(K, V)>,
}

impl<K, V> CacheBuilder<K, V, ()> {
    pub(crate) fn new(clock: Clock) -> Self {
        Self {
            name: None,
            storage: (),
            ttl: None,
            clock,
            telemetry: TelemetryConfig::new(),
            stampede_protection: false,
            _phantom: PhantomData,
        }
    }

    /// Sets a custom storage backend for the cache.
    ///
    /// Use this to provide your own [`CacheTier`] implementation instead of
    /// the built-in options like [`memory()`](Self::memory).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use cachet_memory::InMemoryCache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .storage(InMemoryCache::new())
    ///     .build();
    /// ```
    pub fn storage<CT>(self, storage: CT) -> CacheBuilder<K, V, CT>
    where
        CT: CacheTier<K, V>,
    {
        CacheBuilder {
            name: self.name,
            storage,
            ttl: self.ttl,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }

    /// Configures the cache to use in-memory storage.
    ///
    /// This is the most common storage backend, providing fast concurrent
    /// access with automatic eviction based on capacity.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    /// ```
    #[cfg(feature = "memory")]
    #[must_use]
    pub fn memory(self) -> CacheBuilder<K, V, InMemoryCache<K, V>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        self.storage(InMemoryCache::<K, V>::new())
    }

    /// Configures the cache to use a service as the storage backend.
    ///
    /// This adapts any `Service<CacheOperation>` to work as a `CacheTier`,
    /// enabling remote cache services (Redis, Memcached) or service-based
    /// storage implementations. The service can be composed with middleware
    /// (retry, timeout, circuit breakers) before being wrapped.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .service(redis_service)
    ///     .ttl(Duration::from_secs(300))
    ///     .build();
    /// ```
    #[cfg(feature = "service")]
    #[must_use]
    pub fn service<S>(self, service: S) -> CacheBuilder<K, V, cachet_service::ServiceAdapter<K, V, S>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        S: layered::Service<cachet_service::CacheOperation<K, V>, Out = Result<cachet_service::CacheResponse<V>, crate::Error>>
            + Send
            + Sync,
    {
        self.storage(cachet_service::ServiceAdapter::new(service))
    }
}

impl<K, V, CT> CacheBuilder<K, V, CT> {
    /// Sets a human-readable name for this cache tier, used in telemetry attributes.
    ///
    /// If not set, a name is derived from the storage type.
    ///
    /// Requires `&'static str` because the name is embedded in every telemetry
    /// event (metric labels, log fields). A static reference avoids cloning the
    /// name into a new allocation on each cache operation, which matters at high
    /// throughput. In practice, cache names are always string literals.
    #[must_use]
    pub fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Enables logging for this cache.
    ///
    /// When enabled, cache operations will emit structured logs via the `tracing` crate.
    #[cfg(any(feature = "logs", test))]
    #[must_use]
    pub fn enable_logs(mut self) -> Self {
        self.telemetry = self.telemetry.with_logs();
        self
    }

    /// Configures metrics collection for this cache.
    ///
    /// When configured, cache operations will emit metrics via OpenTelemetry.
    #[cfg(any(feature = "metrics", test))]
    #[must_use]
    pub fn enable_metrics(mut self, meter_provider: &dyn MeterProvider) -> Self {
        self.telemetry = self.telemetry.with_metrics(meter_provider);
        self
    }

    /// Enables stampede protection for cache reads.
    ///
    /// When enabled, concurrent requests for the same key will be merged
    /// so that only one request performs the lookup. Others wait and share the result.
    ///
    /// This prevents the "thundering herd" problem where many concurrent cache
    /// misses for the same key overwhelm the backend.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .stampede_protection()
    ///     .build();
    /// ```
    #[must_use]
    pub fn stampede_protection(mut self) -> Self {
        self.stampede_protection = true;
        self
    }

    /// Sets the time-to-live (TTL) for entries in this cache tier.
    ///
    /// Entries older than the TTL will be considered expired and won't be
    /// returned by get operations. Per-entry TTL in `CacheEntry` overrides
    /// this tier-level setting.
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
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .ttl(Duration::from_secs(300))
    ///     .build();
    /// ```
    #[must_use]
    pub fn ttl(mut self, ttl: impl Into<Duration>) -> Self {
        self.ttl = Some(ttl.into());
        self
    }

    /// Returns a reference to the builder's clock.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }
}

impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Creates a fallback cache with this as the primary tier.
    ///
    /// The primary tier is checked first; on a miss, the fallback tier is queried
    /// and the result is promoted to the primary tier based on the promotion policy.
    ///
    /// Accepts either a `CacheBuilder` or another `FallbackBuilder` as the fallback.
    pub fn fallback<FB>(self, fallback: FB) -> FallbackBuilder<K, V, Self, FB>
    where
        FB: CacheTierBuilder<K, V>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;

        FallbackBuilder {
            name: self.name,
            primary_builder: self,
            fallback_builder: fallback,
            policy: FallbackPromotionPolicy::always(),
            clock,
            refresh: None,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }

    /// Builds the cache with the configured storage and settings.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    /// ```
    pub fn build(self) -> Cache<K, V, CacheWrapper<K, V, CT>> {
        <Self as Buildable<K, V>>::build(self)
    }
}

impl<K, V, CT> Sealed for CacheBuilder<K, V, CT> where CT: CacheTier<K, V> + Send + Sync + 'static {}

impl<K, V, CT> CacheTierBuilder<K, V> for CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tick::Clock;

    use super::*;
    use crate::{Cache, FallbackPromotionPolicy};

    #[test]
    fn type_name_with_user_name() {
        let name = super::super::buildable::type_name::<String>(Some("custom_name"));
        assert_eq!(name, "custom_name");
    }

    #[test]
    fn type_name_without_user_name() {
        let name = super::super::buildable::type_name::<String>(None);
        assert_eq!(name, "alloc::string::String");
    }

    #[test]
    fn cache_builder_with_ttl() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .ttl(Duration::from_secs(300))
            .build();

        assert!(cache.inner().ttl.is_some());
        assert_eq!(cache.inner().ttl, Some(Duration::from_secs(300)));
    }

    #[test]
    fn builder_enable_logs() {
        let clock = Clock::new_frozen();
        let builder = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .enable_logs();
        assert!(builder.telemetry.logs_enabled);
    }

    #[test]
    fn builder_enable_metrics() {
        let clock = Clock::new_frozen();
        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::default();
        let builder = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .enable_metrics(&provider);
        assert!(builder.telemetry.meter.is_some());
    }

    #[test]
    fn mock_builder_new_and_storage() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).storage(cachet_tier::MockCache::new()).build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_name() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .name("my_cache")
            .build();
        assert_eq!(cache.name(), "my_cache");
    }

    #[test]
    fn mock_builder_stampede_protection() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .stampede_protection()
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_clock() {
        let clock = Clock::new_frozen();
        let builder = Cache::builder::<String, i32>(clock).storage(cachet_tier::MockCache::new());
        let _ = builder.clock();
    }

    #[test]
    fn mock_builder_fallback_and_build() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_fallback_promotion_policy() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .promotion_policy(FallbackPromotionPolicy::never())
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_fallback_time_to_refresh() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let refresh = crate::refresh::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .time_to_refresh(refresh)
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_fallback_stampede_protection() {
        let clock = Clock::new_frozen();
        let fb = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(fb)
            .stampede_protection()
            .build();
        assert!(!cache.name().is_empty());
    }

    #[test]
    fn mock_builder_nested_fallback() {
        let clock = Clock::new_frozen();
        let l3 = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let l2 = Cache::builder::<String, i32>(clock.clone())
            .storage(cachet_tier::MockCache::new())
            .fallback(l3);
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(l2)
            .build();
        assert!(!cache.name().is_empty());
    }

    /// Tests `FallbackBuilder::fallback()` - chaining `.fallback()` on a
    /// `FallbackBuilder` (not just `CacheBuilder`).
    #[test]
    fn fallback_builder_fallback() {
        let clock = Clock::new_frozen();
        let l3 = Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new());
        let l2 = Cache::builder::<String, i32>(clock.clone())
            .storage(cachet_tier::MockCache::new())
            .fallback(l3);
        let l1 = l2.fallback(Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new()));
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(l1)
            .build();
        assert!(!cache.name().is_empty());
    }
}
