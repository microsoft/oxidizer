// Copyright (c) Microsoft Corporation.

//! Wrapped Cache Example
//!
//! Demonstrates an in-memory cache with TTL and telemetry.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, CacheTelemetry};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use tick::Clock;

fn setup_telemetry(clock: Clock) -> CacheTelemetry {
    let logger_provider = SdkLoggerProvider::builder().build();
    let meter_provider = SdkMeterProvider::builder().build();

    CacheTelemetry::new(logger_provider, &meter_provider, clock)
}

#[tokio::main]
async fn main() {
    // Set up telemetry (logs cache hits, misses, and operations)
    let clock = Clock::new_tokio();
    let cachelon_telemetry = setup_telemetry(clock.clone());

    // Build a cache with TTL and telemetry
    // - In-memory storage
    // - 30 second TTL
    // - Telemetry records all operations
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .telemetry(cachelon_telemetry, "wrapped-cache")
        .ttl(Duration::from_secs(30))
        .build();

    // Insert a value (telemetry records insert operation)
    let key = "user:123".to_string();
    cache.insert(&key, CacheEntry::new("Alice".to_string())).await;

    // Get existing key (telemetry records cache HIT)
    let _hit = cache.get(&key).await;

    // Get non-existent key (telemetry records cache MISS)
    let missing_key = "user:456".to_string();
    let _miss = cache.get(&missing_key).await;
}
