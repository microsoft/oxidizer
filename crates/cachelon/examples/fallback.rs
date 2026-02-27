// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Two-tier cache with automatic fallback and promotion.
//! On L1 miss, L2 is checked and the result is promoted to L1.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // L2: fallback cache (longer TTL)
    let l2 = Cache::builder::<String, String>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(300));

    // L1: primary cache (shorter TTL) with L2 fallback
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    let key = "user:1".to_string();

    // Insert goes to both L1 and L2
    cache
        .insert(&key, CacheEntry::new("Alice".to_string()))
        .await
        .expect("insert failed");

    // Get from L1
    let value = cache.get(&key).await.expect("get failed");
    match value {
        Some(e) => println!("get({key}): {}", e.value()),
        None => println!("get({key}): not found"),
    }

    // Invalidate only clears L1; L2 still has it
    // (Next get would promote from L2 back to L1)
}
