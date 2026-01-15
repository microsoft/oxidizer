// Copyright (c) Microsoft Corporation.

//! Fallback Cache Example
//!
//! Demonstrates a two-tier cache with automatic fallback and promotion.
//! The primary cache is checked first, and on a miss, the fallback is consulted.
//! Values found in the fallback are automatically promoted to the primary cache.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, CacheTelemetry, FallbackPromotionPolicy};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use tick::Clock;

fn setup_telemetry(clock: Clock) -> CacheTelemetry {
    let logger_provider = SdkLoggerProvider::builder().build();
    let meter_provider = SdkMeterProvider::builder().build();

    CacheTelemetry::new(logger_provider, &meter_provider, clock)
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let cachelon_telemetry = setup_telemetry(clock.clone());

    basic_fallback(&clock, &cachelon_telemetry).await;
    promotion_with_when(&clock).await;
    promotion_with_when_capturing(&clock).await;
}

/// Basic fallback example with `FallbackPromotionPolicy::Always`.
async fn basic_fallback(clock: &Clock, cachelon_telemetry: &CacheTelemetry) {
    // Build a two-tier cache:
    // - Primary: fast in-memory cache with short TTL (60 seconds)
    // - Fallback: slower cache with longer TTL (3600 seconds)
    // - Policy: Always promote from fallback to primary
    let fallback = Cache::builder::<String, String>(clock.clone())
        .memory()
        .telemetry(cachelon_telemetry.clone(), "fallback")
        .ttl(Duration::from_secs(3600));

    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .telemetry(cachelon_telemetry.clone(), "primary")
        .ttl(Duration::from_secs(60))
        .with_fallback(fallback)
        .promotion_policy(FallbackPromotionPolicy::Always)
        .build();

    // Insert value (goes to both primary and fallback)
    let key = "config:app".to_string();
    cache.insert(&key, CacheEntry::new("production".to_string())).await;

    // Get existing key (hits primary cache)
    let _value = cache.get(&key).await;

    // Get non-existent key (misses both caches)
    let missing_key = "config:missing".to_string();
    let _missing = cache.get(&missing_key).await;
}

/// Example using `FallbackPromotionPolicy::When` with a static predicate (no captures).
async fn promotion_with_when(clock: &Clock) {
    // Predicate: only promote values that are not empty
    fn not_empty(entry: &CacheEntry<String>) -> bool {
        !entry.value().is_empty()
    }

    // Build cache with conditional promotion policy
    // Uses a function pointer (most efficient when no captures needed)
    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .with_fallback(Cache::builder(clock.clone()).memory())
        .promotion_policy(FallbackPromotionPolicy::when(not_empty))
        .build();

    // Insert non-empty value
    let key = "key1".to_string();
    cache.insert(&key, CacheEntry::new("non-empty value".to_string())).await;

    // Get value (would promote if found in fallback)
    let _value = cache.get(&key).await;
}

/// Example using `FallbackPromotionPolicy::when_boxed` with a closure that captures state.
async fn promotion_with_when_capturing(clock: &Clock) {
    // Promotion rule: only promote values matching a runtime-determined prefix
    let required_prefix = "prod_".to_string();

    // Build cache with conditional promotion using closure
    // when_boxed() accepts closures that capture external state
    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .with_fallback(Cache::builder(clock.clone()).memory())
        .promotion_policy(FallbackPromotionPolicy::when_boxed(move |entry: &CacheEntry<String>| {
            entry.value().starts_with(&required_prefix)
        }))
        .build();

    // Insert production config (starts with 'prod_' - would be promoted)
    let prod_key = "config1".to_string();
    cache.insert(&prod_key, CacheEntry::new("prod_database_url".to_string())).await;

    // Insert development config (starts with 'dev_' - would NOT be promoted)
    let dev_key = "config2".to_string();
    cache.insert(&dev_key, CacheEntry::new("dev_database_url".to_string())).await;

    // Get both values (prod_key would promote from fallback, dev_key would not)
    let _prod_value = cache.get(&prod_key).await;
    let _dev_value = cache.get(&dev_key).await;
}
