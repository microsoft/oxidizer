// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache with TTL and telemetry (logs hits, misses, operations).

use std::time::Duration;

use cachelon::{Cache, CacheEntry, CacheTelemetry};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Set up telemetry
    let logger = SdkLoggerProvider::builder().build();
    let meter = SdkMeterProvider::builder().build();
    let telemetry = CacheTelemetry::new(logger, &meter, clock.clone());

    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .telemetry(telemetry, "my-cache")
        .ttl(Duration::from_secs(30))
        .build();

    let key = "user:1".to_string();

    cache
        .insert(&key, CacheEntry::new("Alice".to_string()))
        .await
        .expect("insert failed");
    println!("insert: ok");

    let hit = cache.get(&key).await.expect("get failed");
    println!("get (hit): {:?}", hit.map(|e| e.value().clone()));

    let miss = cache.get(&"missing".to_string()).await.expect("get failed");
    println!("get (miss): {miss:?}");
}
