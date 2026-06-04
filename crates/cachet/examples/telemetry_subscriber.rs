// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates cachet telemetry as structured tracing events.
//!
//! Run with: `cargo run --example telemetry_subscriber --features "memory,logs"`

use std::time::Duration;

use cachet::{Cache, CacheEntry};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;

#[tokio::main]
async fn main() {
    // Set up a subscriber that shows cachet's structured events.
    let subscriber = tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_ansi(true).with_target(false));
    tracing::subscriber::set_global_default(subscriber).expect("subscriber already set");

    let clock = Clock::new_tokio();
    let cache: Cache<String, String> = Cache::builder(clock).memory().enable_logs().ttl(Duration::from_secs(30)).build();

    println!("--- Insert ---");
    cache
        .insert("user:1".to_string(), CacheEntry::new("Alice".to_string()))
        .await
        .expect("insert failed");

    println!("\n--- Get (hit) ---");
    let _ = cache.get(&"user:1".to_string()).await;

    println!("\n--- Get (miss) ---");
    let _ = cache.get(&"user:999".to_string()).await;
}
