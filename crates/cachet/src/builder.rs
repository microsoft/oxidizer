// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache builder types for constructing single and multi-tier caches.
//!
//! This module provides the builder pattern infrastructure for creating
//! caches with configurable storage, TTL, telemetry, and fallback tiers.

use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

#[cfg(feature = "memory")]
use cachet_memory::InMemoryCache;
use cachet_tier::{DynamicCache, DynamicCacheExt};
#[cfg(any(feature = "metrics", test))]
use opentelemetry::metrics::MeterProvider;
use tick::Clock;

use crate::builder::sealed::Sealed;
use crate::fallback::{FallbackCache, FallbackPromotionPolicy};
use crate::refresh::TimeToRefresh;
use crate::telemetry::{CacheTelemetry, TelemetryConfig};
use crate::wrapper::CacheWrapper;
use crate::{Cache, CacheTier};

mod sealed {
    pub(crate) trait Sealed {}
}

/// A builder that can produce a cache tier.
///
/// This trait is sealed and cannot be implemented outside this crate.
/// It's implemented by `CacheBuilder` and `FallbackBuilder` to enable
/// type-safe cache hierarchy construction.
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
#[expect(private_bounds, reason = "intentionally sealed trait pattern")]
pub trait CacheTierBuilder<K, V>: Sealed {}

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
    name: Option<&'static str>,
    storage: CT,
    ttl: Option<Duration>,
    clock: Clock,
    telemetry: TelemetryConfig,
    stampede_protection: bool,
    _phantom: PhantomData<(K, V)>,
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
    pub fn use_logs(mut self) -> Self {
        self.telemetry = self.telemetry.with_logs();
        self
    }

    /// Configures metrics collection for this cache.
    ///
    /// When configured, cache operations will emit metrics via OpenTelemetry.
    #[cfg(any(feature = "metrics", test))]
    #[must_use]
    pub fn use_metrics(mut self, meter_provider: &dyn MeterProvider) -> Self {
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

/// Builder for a cache with fallback tiers.
///
/// Created via `CacheBuilder::fallback`. When built, produces a `Cache`
/// wrapping a `FallbackCache` structure.
#[derive(Debug)]
#[expect(clippy::struct_field_names, reason = "builder field naming is intentional")]
pub struct FallbackBuilder<K, V, PB, FB> {
    name: Option<&'static str>,
    primary_builder: PB,
    fallback_builder: FB,
    policy: FallbackPromotionPolicy<V>,
    clock: Clock,
    refresh: Option<TimeToRefresh<K>>,
    telemetry: TelemetryConfig,
    stampede_protection: bool,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB> {
    /// Sets the promotion policy for this fallback tier.
    ///
    /// The policy determines when values from the fallback tier should be
    /// promoted to the primary tier.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::{Cache, FallbackPromotionPolicy};
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .fallback(l2)
    ///     .promotion_policy(FallbackPromotionPolicy::always())
    ///     .build();
    /// ```
    #[must_use]
    pub fn promotion_policy(mut self, policy: FallbackPromotionPolicy<V>) -> Self {
        self.policy = policy;
        self
    }

    /// Configures background refresh for this fallback tier.
    ///
    /// When entries in the primary tier exceed the refresh duration,
    /// they will be asynchronously refreshed from the fallback tier.
    #[must_use]
    pub fn time_to_refresh(mut self, refresh: TimeToRefresh<K>) -> Self {
        self.refresh = Some(refresh);
        self
    }

    /// Enables stampede protection for cache reads.
    ///
    /// When enabled, concurrent requests for the same key will be merged
    /// so that only one request performs the lookup. Others wait and share the result.
    ///
    /// This prevents the "thundering herd" problem where many concurrent cache
    /// misses for the same key overwhelm the backend.
    #[must_use]
    pub fn stampede_protection(mut self) -> Self {
        self.stampede_protection = true;
        self
    }
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Adds another fallback tier to the cache hierarchy.
    ///
    /// This allows building arbitrarily deep cache hierarchies like:
    /// L1 → L2 → L3 → Database
    ///
    /// Each `FallbackBuilder` controls its own promotion policy via `.promotion_policy()`.
    ///
    /// Accepts either a `CacheBuilder` or another `FallbackBuilder` as the fallback.
    pub fn fallback<FB2>(self, fallback: FB2) -> FallbackBuilder<K, V, Self, FB2>
    where
        FB2: CacheTierBuilder<K, V>,
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
}

impl<K, V, PB, FB> Sealed for FallbackBuilder<K, V, PB, FB>
where
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
}

impl<K, V, PB, FB> CacheTierBuilder<K, V> for FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
}

#[expect(private_bounds, reason = "Buildable is an internal trait")]
impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V> + Buildable<K, V>,
    FB: CacheTierBuilder<K, V> + Buildable<K, V>,
{
    /// Builds the multi-tier cache hierarchy.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let l2 = Cache::builder::<String, i32>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .fallback(l2)
    ///     .build();
    /// ```
    pub fn build(self) -> Cache<K, V, DynamicCache<K, V>> {
        <Self as Buildable<K, V>>::build(self)
    }
}

/// Internal trait for building cache hierarchies.
pub(crate) trait Buildable<K, V> {
    type Output: CacheTier<K, V> + Send + Sync + 'static;
    type TierOutput: CacheTier<K, V> + Send + Sync + 'static;

    fn build(self) -> Cache<K, V, Self::Output>;

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::TierOutput;
}

impl<K, V, CT> Buildable<K, V> for CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    type Output = CacheWrapper<K, V, CT>;
    type TierOutput = Self::Output;

    fn build(self) -> Cache<K, V, Self::Output> {
        let name = self.name;
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone().build();
        let stampede_protection = self.stampede_protection;

        let tier = self.build_tier(clock.clone(), telemetry);

        Cache::new(type_name::<Cache<K, V, Self::TierOutput>>(name), tier, clock, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::TierOutput {
        CacheWrapper::new(type_name::<CT>(self.name), self.storage, clock, self.ttl, telemetry)
    }
}

impl<K, V, PB, FB> Buildable<K, V> for FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: Buildable<K, V>,
    FB: Buildable<K, V>,
{
    type Output = DynamicCache<K, V>;
    type TierOutput = FallbackCache<K, V, PB::TierOutput, FB::TierOutput>;

    fn build(self) -> Cache<K, V, Self::Output> {
        let name = self.name;
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone().build();
        let stampede_protection = self.stampede_protection;

        let tier = self.build_tier(clock.clone(), telemetry).into_dynamic();

        Cache::new(type_name::<Cache<K, V, Self::TierOutput>>(name), tier, clock, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::TierOutput {
        let primary = self.primary_builder.build_tier(clock.clone(), telemetry.clone());
        let fallback = self.fallback_builder.build_tier(clock.clone(), telemetry.clone());

        FallbackCache::new(
            type_name::<Self::TierOutput>(self.name),
            primary,
            fallback,
            self.policy,
            clock,
            self.refresh,
            telemetry,
        )
    }
}

fn type_name<S>(user_name: Option<&'static str>) -> &'static str {
    if let Some(name) = user_name {
        name
    } else {
        std::any::type_name::<S>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_name_with_user_name() {
        let name = type_name::<String>(Some("custom_name"));
        assert_eq!(name, "custom_name");
    }

    #[test]
    fn type_name_without_user_name() {
        let name = type_name::<String>(None);
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
    fn builder_use_logs() {
        let clock = Clock::new_frozen();
        let builder = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .use_logs();
        assert_eq!(builder.telemetry.logs_enabled, true);
    }

    #[test]
    fn builder_use_metrics() {
        let clock = Clock::new_frozen();
        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::default();
        let builder = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .use_metrics(&provider);
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
        // Stampede protection is internal; just verify it builds
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
        // l2 is a FallbackBuilder
        let l2 = Cache::builder::<String, i32>(clock.clone())
            .storage(cachet_tier::MockCache::new())
            .fallback(l3);
        // Call fallback() on FallbackBuilder to exercise FallbackBuilder::fallback
        let l1 = l2.fallback(Cache::builder::<String, i32>(clock.clone()).storage(cachet_tier::MockCache::new()));
        let cache = Cache::builder::<String, i32>(clock)
            .storage(cachet_tier::MockCache::new())
            .fallback(l1)
            .build();
        assert!(!cache.name().is_empty());
    }
}
