// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::Hash;

use cachet_tier::DynamicCache;
use tick::Clock;

use super::cache::CacheBuilder;
use super::fallback::FallbackBuilder;
use crate::fallback::FallbackCache;
use crate::telemetry::CacheTelemetry;
use crate::wrapper::CacheWrapper;
use crate::{Cache, CacheTier};

/// Internal trait for building cache hierarchies.
pub(super) trait Buildable<K, V> {
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
        CacheWrapper::new(type_name::<CT>(self.name), self.storage, clock, self.ttl, telemetry, self.policy)
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

        let tier = DynamicCache::new(self.build_tier(clock.clone(), telemetry));

        Cache::new(type_name::<Cache<K, V, Self::TierOutput>>(name), tier, clock, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::TierOutput {
        let primary = self.primary_builder.build_tier(clock.clone(), telemetry.clone());
        let fallback = self.fallback_builder.build_tier(clock.clone(), telemetry.clone());

        FallbackCache::new(
            type_name::<Self::TierOutput>(self.name),
            primary,
            fallback,
            clock,
            self.refresh,
            telemetry,
        )
    }
}

pub(super) fn type_name<S>(user_name: Option<&'static str>) -> &'static str {
    user_name.unwrap_or_else(|| std::any::type_name::<S>())
}
