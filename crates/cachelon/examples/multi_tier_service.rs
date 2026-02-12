// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier cache: L1 in-memory + L2 service-based.
//!
//! This pattern is common when L2 is a shared remote cache (Redis)
//! that you want to wrap with middleware.

use std::time::Duration;

use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
use cachelon_tier::testing::MockCache;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();

    // L2: Simulated remote cache (in practice: Redis via .service())
    let l2_storage = MockCache::<String, String>::new();
    let l2 = Cache::builder::<String, String>(clock.clone())
        .storage(l2_storage)
        .ttl(Duration::from_secs(600));

    // L1: in-memory (shorter TTL, local)
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    cache
        .insert(&"key".to_string(), CacheEntry::new("value".to_string()))
        .await
        .expect("insert failed");

    let v = cache.get(&"key".to_string()).await.expect("get failed");
    match v {
        Some(e) => println!("get(key): {}", e.value()),
        None => println!("get(key): not found"),
    }
}
