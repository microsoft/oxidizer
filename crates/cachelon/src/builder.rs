// Copyright (c) Microsoft Corporation.

//! Cache builder types for constructing single and multi-tier caches.
//!
//! This module provides the builder pattern infrastructure for creating
//! caches with configurable storage, TTL, telemetry, and fallback tiers.

use std::{hash::Hash, marker::PhantomData, time::Duration};
use tick::Clock;

#[cfg(any(feature = "tokio", test))]
use crate::refresh::TimeToRefresh;
use crate::{
    Cache, CacheTier,
    builder::sealed::Sealed,
    fallback::{FallbackCache, FallbackPromotionPolicy},
    wrapper::CacheWrapper,
};

#[cfg(feature = "telemetry")]
use crate::telemetry::CacheTelemetry;
#[cfg(feature = "memory")]
use cachelon_memory::InMemoryCache;

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
/// ```
/// use cachelon::Cache;
/// use tick::Clock;
///
/// let clock = Clock::new_frozen();
/// let cache = Cache::builder::<String, i32>(clock)
///     .memory()
///     .build();
/// ```
#[expect(private_bounds, reason = "intentionally sealed trait pattern")]
pub trait CacheTierBuilder<K, V>: Sealed {
    /// The output tier type produced by this builder.
    type Tier;
}

/// Builder for constructing a cache with a single tier.
///
/// Created by calling `Cache::builder()`. Allows configuring storage,
/// TTL, telemetry, and adding fallback tiers.
///
/// # Examples
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
pub struct CacheBuilder<K, V, S = ()> {
    name: Option<&'static str>,
    storage: S,
    ttl: Option<Duration>,
    clock: Clock,
    #[cfg(feature = "telemetry")]
    telemetry: Option<CacheTelemetry>,
    #[cfg(feature = "tokio")]
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
            #[cfg(feature = "telemetry")]
            telemetry: None,
            #[cfg(feature = "tokio")]
            stampede_protection: false,
            _phantom: PhantomData,
        }
    }

    /// Sets a custom storage backend for the cache.
    ///
    /// Use this to provide your own `CacheTier` implementation instead of
    /// the built-in options like `memory()`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "test-util")]
    /// # fn main() {
    /// use cachelon::{Cache, MockCache};
    /// use tick::Clock;
    ///
    /// let mock = MockCache::<String, i32>::new();
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .storage(mock)
    ///     .build();
    /// # }
    /// # #[cfg(not(feature = "test-util"))]
    /// # fn main() {}
    /// ```
    pub fn storage<S>(self, storage: S) -> CacheBuilder<K, V, S>
    where
        S: CacheTier<K, V>,
    {
        CacheBuilder {
            name: self.name,
            storage,
            ttl: self.ttl,
            clock: self.clock,
            #[cfg(feature = "telemetry")]
            telemetry: self.telemetry,
            #[cfg(feature = "tokio")]
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
    /// ```
    /// use cachelon::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .build();
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
    #[expect(
        clippy::wrong_self_convention,
        reason = "builder method that consumes self to construct storage from service"
    )]
    pub fn service<S>(self, service: S) -> CacheBuilder<K, V, cachelon_service::ServiceAdapter<K, V, S>>
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        S: layered::Service<cachelon_service::CacheOperation<K, V>, Out = Result<cachelon_service::CacheResponse<V>, crate::Error>>
            + Send
            + Sync,
    {
        self.storage(cachelon_service::ServiceAdapter::new(service))
    }
}

impl<K, V, S> CacheBuilder<K, V, S> {
    /// Sets the telemetry and name for this cache tier.
    ///
    /// The name is used to identify this tier in telemetry output.
    #[cfg(feature = "telemetry")]
    #[must_use]
    pub fn telemetry(mut self, telemetry: CacheTelemetry, name: &'static str) -> Self {
        self.telemetry = Some(telemetry);
        self.name = Some(name);
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
    /// ```
    /// use cachelon::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .stampede_protection()
    ///     .build();
    /// ```
    #[cfg(feature = "tokio")]
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
    /// ```
    /// use cachelon::Cache;
    /// use tick::Clock;
    /// use std::time::Duration;
    ///
    /// let clock = Clock::new_frozen();
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

impl<K, V, S> CacheBuilder<K, V, S>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync + 'static,
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
        #[cfg(feature = "telemetry")]
        let telemetry = self.telemetry.clone();
        #[cfg(feature = "tokio")]
        let stampede_protection = self.stampede_protection;

        FallbackBuilder {
            name: self.name,
            primary_builder: self,
            fallback_builder: fallback,
            policy: FallbackPromotionPolicy::Always,
            clock,
            #[cfg(any(feature = "tokio", test))]
            refresh: None,
            #[cfg(feature = "telemetry")]
            telemetry,
            #[cfg(feature = "tokio")]
            stampede_protection,
            _phantom: PhantomData,
        }
    }

    /// Builds the cache with the configured storage and settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .build();
    /// ```
    pub fn build(self) -> Cache<K, V, CacheWrapper<K, V, S>> {
        <Self as Buildable<K, V>>::build(self)
    }
}

impl<K, V, S> Sealed for CacheBuilder<K, V, S> where S: CacheTier<K, V> + Send + Sync + 'static {}

impl<K, V, S> CacheTierBuilder<K, V> for CacheBuilder<K, V, S>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync + 'static,
{
    type Tier = CacheWrapper<K, V, S>;
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
    #[cfg(any(feature = "tokio", test))]
    refresh: Option<TimeToRefresh<K>>,
    #[cfg(feature = "telemetry")]
    telemetry: Option<CacheTelemetry>,
    #[cfg(feature = "tokio")]
    stampede_protection: bool,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB> {
    /// Sets the telemetry and name for this fallback cache.
    ///
    /// The name is used to identify this tier in telemetry output.
    #[cfg(feature = "telemetry")]
    #[must_use]
    pub fn telemetry(mut self, telemetry: CacheTelemetry, name: &'static str) -> Self {
        self.telemetry = Some(telemetry);
        self.name = Some(name);
        self
    }

    /// Sets the promotion policy for this fallback tier.
    ///
    /// The policy determines when values from the fallback tier should be
    /// promoted to the primary tier.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, FallbackPromotionPolicy};
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_frozen();
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
    #[cfg(any(feature = "tokio", test))]
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
    #[cfg(feature = "tokio")]
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
    /// L1 -> L2 -> L3 -> Database
    ///
    /// Each `FallbackBuilder` controls its own promotion policy via `.promotion_policy()`.
    ///
    /// Accepts either a `CacheBuilder` or another `FallbackBuilder` as the fallback.
    pub fn fallback<FB2>(self, fallback: FB2) -> FallbackBuilder<K, V, Self, FB2>
    where
        FB2: CacheTierBuilder<K, V>,
    {
        let clock = self.clock.clone();
        #[cfg(feature = "telemetry")]
        let telemetry = self.telemetry.clone();
        #[cfg(feature = "tokio")]
        let stampede_protection = self.stampede_protection;

        FallbackBuilder {
            name: self.name,
            primary_builder: self,
            fallback_builder: fallback,
            policy: FallbackPromotionPolicy::Always,
            clock,
            #[cfg(any(feature = "tokio", test))]
            refresh: None,
            #[cfg(feature = "telemetry")]
            telemetry,
            #[cfg(feature = "tokio")]
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
    type Tier = FallbackCache<K, V, PB::Tier, FB::Tier>;
}

#[expect(private_bounds, reason = "Buildable is an internal trait")]
#[expect(clippy::type_complexity, reason = "complex type is unavoidable for builder pattern")]
impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V> + Buildable<K, V, Output = PB::Tier>,
    FB: CacheTierBuilder<K, V> + Buildable<K, V, Output = FB::Tier>,
{
    /// Builds the multi-tier cache hierarchy.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_frozen();
    /// let l2 = Cache::builder::<String, i32>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .fallback(l2)
    ///     .build();
    /// ```
    pub fn build(self) -> Cache<K, V, FallbackCache<K, V, PB::Tier, FB::Tier>> {
        <Self as Buildable<K, V>>::build(self)
    }
}

/// Internal trait for building cache hierarchies.
pub(crate) trait Buildable<K, V> {
    type Output: CacheTier<K, V> + Send + Sync + 'static;

    fn build(self) -> Cache<K, V, Self::Output>;

    fn build_tier(self, clock: Clock, #[cfg(feature = "telemetry")] telemetry: Option<CacheTelemetry>) -> Self::Output;
}

impl<K, V, S> Buildable<K, V> for CacheBuilder<K, V, S>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync + 'static,
{
    type Output = CacheWrapper<K, V, S>;

    fn build(self) -> Cache<K, V, Self::Output> {
        let clock = self.clock.clone();
        #[cfg(feature = "telemetry")]
        let telemetry = self.telemetry.clone();
        #[cfg(feature = "tokio")]
        let stampede_protection = self.stampede_protection;

        let tier = self.build_tier(
            clock.clone(),
            #[cfg(feature = "telemetry")]
            telemetry,
        );

        Cache::new(
            short_type_name::<Cache<K, V, Self::Output>>(None),
            tier,
            clock,
            #[cfg(feature = "tokio")]
            stampede_protection,
        )
    }

    fn build_tier(self, clock: Clock, #[cfg(feature = "telemetry")] telemetry: Option<CacheTelemetry>) -> Self::Output {
        CacheWrapper::new(
            short_type_name::<S>(self.name),
            self.storage,
            clock,
            self.ttl,
            #[cfg(feature = "telemetry")]
            telemetry,
        )
    }
}

impl<K, V, PB, FB> Buildable<K, V> for FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: Buildable<K, V>,
    FB: Buildable<K, V>,
{
    type Output = FallbackCache<K, V, PB::Output, FB::Output>;

    fn build(self) -> Cache<K, V, Self::Output> {
        let name = self.name;
        let clock = self.clock.clone();
        #[cfg(feature = "telemetry")]
        let telemetry = self.telemetry.clone();
        #[cfg(feature = "tokio")]
        let stampede_protection = self.stampede_protection;

        let tier = self.build_tier(
            clock.clone(),
            #[cfg(feature = "telemetry")]
            telemetry,
        );

        Cache::new(
            short_type_name::<Cache<K, V, Self::Output>>(name),
            tier,
            clock,
            #[cfg(feature = "tokio")]
            stampede_protection,
        )
    }

    fn build_tier(self, clock: Clock, #[cfg(feature = "telemetry")] telemetry: Option<CacheTelemetry>) -> Self::Output {
        let primary = self.primary_builder.build_tier(
            clock.clone(),
            #[cfg(feature = "telemetry")]
            telemetry.clone(),
        );
        let fallback = self.fallback_builder.build_tier(
            clock.clone(),
            #[cfg(feature = "telemetry")]
            telemetry.clone(),
        );

        FallbackCache::new(
            short_type_name::<Self::Output>(self.name),
            primary,
            fallback,
            self.policy,
            clock,
            #[cfg(any(feature = "tokio", test))]
            self.refresh,
            #[cfg(feature = "telemetry")]
            telemetry,
        )
    }
}

fn short_type_name<S>(user_name: Option<&'static str>) -> &'static str {
    if let Some(name) = user_name {
        name
    } else {
        let full = std::any::type_name::<S>();
        full.rsplit("::").next().unwrap_or(full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_type_name_with_user_name() {
        let name = short_type_name::<String>(Some("custom_name"));
        assert_eq!(name, "custom_name");
    }

    #[test]
    fn short_type_name_without_user_name() {
        let name = short_type_name::<String>(None);
        assert_eq!(name, "String");
    }

    #[test]
    fn cachelon_builder_with_ttl() {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().ttl(Duration::from_secs(300)).build();

        assert!(cache.inner().ttl.is_some());
        assert_eq!(cache.inner().ttl, Some(Duration::from_secs(300)));
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn cachelon_builder_with_telemetry() {
        use crate::telemetry::CacheTelemetry;
        use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};

        let clock = Clock::new_frozen();
        let logger_provider = SdkLoggerProvider::builder().build();
        let meter_provider = SdkMeterProvider::builder().build();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock.clone());

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .telemetry(telemetry, "test_cache")
            .build();

        assert_eq!(cache.inner().name(), "test_cache");
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn fallback_builder_with_telemetry() {
        use crate::telemetry::CacheTelemetry;
        use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};

        let clock = Clock::new_frozen();
        let logger_provider = SdkLoggerProvider::builder().build();
        let meter_provider = SdkMeterProvider::builder().build();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock.clone());

        let fallback = Cache::builder::<String, i32>(clock.clone()).memory();

        let cache = Cache::builder::<String, i32>(clock)
            .memory()
            .fallback(fallback)
            .telemetry(telemetry, "fallback_cache")
            .build();

        assert_eq!(cache.name(), "fallback_cache");
    }

    #[test]
    fn fallback_builder_nested_fallback() {
        let clock = Clock::new_frozen();

        // L3 (deepest)
        let l3 = Cache::builder::<String, i32>(clock.clone()).memory();

        // L2 with its own fallback
        let l2 = Cache::builder::<String, i32>(clock.clone()).memory().fallback(l3);

        // L1 with nested fallback - using FallbackBuilder.fallback
        let cache = Cache::builder::<String, i32>(clock).memory().fallback(l2).build();

        assert!(!cache.name().is_empty());
    }
}
