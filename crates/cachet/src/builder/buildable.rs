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
pub(crate) trait Buildable<K, V> {
    type TierOutput: CacheTier<K, V> + Send + Sync + 'static;

    fn build(self) -> Cache<K, V>;

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry, fallback: bool) -> Self::TierOutput;
}

impl<K, V, CT> Buildable<K, V> for CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    type TierOutput = CacheWrapper<K, V, CT>;

    fn build(self) -> Cache<K, V> {
        let name = self.name;
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;

        let tier = DynamicCache::new(self.build_tier(clock.clone(), telemetry.clone(), false));

        Cache::new(type_name::<Self::TierOutput>(name), tier, clock, telemetry, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry, fallback: bool) -> Self::TierOutput {
        let name = type_name::<CT>(self.name);
        #[cfg(feature = "memory")]
        if let Some(hook) = &self.eviction_hook {
            hook.init(telemetry.clone(), name);
        }
        CacheWrapper::new(name, self.storage, clock, self.ttl, telemetry, self.policy, fallback)
    }
}

impl<K, V, PB, FB> Buildable<K, V> for FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: Buildable<K, V>,
    FB: Buildable<K, V>,
{
    type TierOutput = FallbackCache<K, V, PB::TierOutput, FB::TierOutput>;

    fn build(self) -> Cache<K, V> {
        let name = self.name;
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;

        let tier = DynamicCache::new(self.build_tier(clock.clone(), telemetry.clone(), false));

        Cache::new(type_name::<Self::TierOutput>(name), tier, clock, telemetry, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry, fallback: bool) -> Self::TierOutput {
        let primary = self.primary_builder.build_tier(clock.clone(), telemetry.clone(), fallback);
        let fallback = self.fallback_builder.build_tier(clock.clone(), telemetry.clone(), true);

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

pub(crate) fn type_name<S>(user_name: Option<&'static str>) -> &'static str {
    user_name.unwrap_or_else(|| std::any::type_name::<S>())
}
