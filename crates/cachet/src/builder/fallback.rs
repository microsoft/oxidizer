// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

use cachet_tier::DynamicCache;
use tick::Clock;

use super::buildable::Buildable;
use super::cache::CacheBuilder;
use super::sealed::{CacheTierBuilder, Sealed};
use crate::fallback::{FallbackCache, FallbackPromotionPolicy};
use crate::refresh::TimeToRefresh;
use crate::telemetry::TelemetryConfig;
use crate::{Cache, CacheTier};

/// Builder for a cache with fallback tiers.
///
/// Created via `CacheBuilder::fallback`. When built, produces a `Cache`
/// wrapping a `FallbackCache` structure.
#[derive(Debug)]
#[expect(clippy::struct_field_names, reason = "builder field naming is intentional")]
pub struct FallbackBuilder<K, V, PB, FB> {
    pub(crate) name: Option<&'static str>,
    pub(crate) primary_builder: PB,
    pub(crate) fallback_builder: FB,
    pub(crate) policy: FallbackPromotionPolicy<V>,
    pub(crate) clock: Clock,
    pub(crate) refresh: Option<TimeToRefresh<K>>,
    pub(crate) telemetry: TelemetryConfig,
    pub(crate) stampede_protection: bool,
    pub(crate) _phantom: PhantomData<(K, V)>,
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
