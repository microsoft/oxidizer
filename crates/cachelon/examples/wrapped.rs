// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache with TTL and telemetry (logs hits, misses, operations).

use std::time::Duration;

use cachelon::{Cache, CacheEntry};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Set up telemetry
    let meter_provider = SdkMeterProvider::builder().build();

    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .use_logs()
        .use_metrics(&meter_provider)
        .ttl(Duration::from_secs(30))
        .build();

    let key = "user:1".to_string();

    cache
        .insert(&key, CacheEntry::new("Alice".to_string()))
        .await
        .expect("insert failed");
    println!("insert: ok");

    let hit = cache.get(&key).await.expect("get failed");
    match hit {
        Some(e) => println!("get (hit): {}", e.value()),
        None => println!("get (hit): not found"),
    }

    let miss = cache.get(&"missing".to_string()).await.expect("get failed");
    match miss {
        Some(e) => println!("get (miss): {}", e.value()),
        None => println!("get (miss): not found"),
    }
}
