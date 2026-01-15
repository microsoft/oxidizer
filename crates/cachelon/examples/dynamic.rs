// Copyright (c) Microsoft Corporation.

//! Dynamic Cache Example
//!
//! Demonstrates how to use `DynamicCache` to simplify complex storage type signatures.
//!
//! Without dynamic dispatch, a multi-tier storage has a complex nested type:
//! ```text
//! FallbackCache<K, V, CacheWrapper<...>, CacheWrapper<...>>
//! ```
//!
//! With `DynamicCache`, the storage becomes simply:
//! ```text
//! DynamicCache<K, V>
//! ```
//!
//! The top-level `Cache` then wraps it: `Cache<K, V, DynamicCache<K, V>>`
//!
//! Trade-off: Dynamic dispatch adds ~60-100ns overhead per operation due to
//! boxing futures. This is negligible for I/O-bound caches but may matter
//! for extremely hot in-memory-only caches.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, CacheTelemetry, DynamicCache, DynamicCacheExt};
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

    // Build a multi-tier cache using the builder pattern
    // L1 (primary) -> L2 (fallback)
    let l2_builder = Cache::builder::<String, String>(clock.clone()).memory();

    let l1_builder = Cache::builder::<String, String>(clock.clone()).memory().with_fallback(l2_builder);

    // At this point, building would give us a complex type:
    // Cache<String, String, FallbackCache<..., CacheWrapper<..., InMemoryCache<...>>, CacheWrapper<..., InMemoryCache<...>>>>

    // Build the fallback storage and convert to DynamicCache for type erasure
    let fallback_cache = l1_builder.build();

    // The original cache has a complex nested type
    let _original_type_name = std::any::type_name_of_val(&fallback_cache);

    // Get the inner storage and convert to DynamicCache
    // Note: This requires consuming the cache to access its storage
    let dynamic_storage: DynamicCache<String, String> = fallback_cache.into_inner().into_dynamic();

    // The dynamic storage has a simple type: DynamicCache<String, String>
    let _dynamic_type_name = std::any::type_name_of_val(&dynamic_storage);

    // Wrap in a top-level Cache for the full API (TTL, stampede protection, etc.)
    let cache = Cache::builder::<String, String>(clock.clone())
        .storage(dynamic_storage.clone())
        .telemetry(cachelon_telemetry, "dynamic-cache")
        .ttl(Duration::from_secs(60))
        .build();

    // Use the cache - full API available
    cache.insert(&"user:1".to_string(), CacheEntry::new("Alice".to_string())).await;
    let _user = cache.get(&"user:1".to_string()).await;

    // DynamicCache is Clone, so you can share the storage across multiple Cache instances
    let _cachelon_clone = dynamic_storage.clone();
}
