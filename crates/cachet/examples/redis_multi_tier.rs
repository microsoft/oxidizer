// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier cache with Redis (L2) and in-memory (L1), plus seatbelt resilience.
//!
//! Requires a running Redis instance on `redis://127.0.0.1/`.
//! Start one with: `docker compose up -d`

use std::time::Duration;

use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
use cachet_redis::RedisCache;
use cachet_tier::CacheTier;
use layered::Layer;
use seatbelt::{RecoveryInfo, ResilienceContext, retry::Retry, timeout::Timeout};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Connect to Redis
    let client = redis::Client::open("redis://127.0.0.1/").expect("invalid redis URL");
    let conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("failed to connect to Redis — is it running? try: docker compose up -d");

    // ── Part 1: Multi-tier fallback (L1 memory + L2 Redis) ──────────────

    let redis_cache = RedisCache::<String, String>::builder(conn.clone())
        .key_prefix("example:")
        .build();

    // Seed a value directly into Redis (L2)
    redis_cache
        .insert(
            &"greeting".to_string(),
            CacheEntry::new("hello from redis".to_string()),
        )
        .await
        .expect("seed insert failed");

    // L2: Redis-backed tier
    let l2 = Cache::builder::<String, String>(clock.clone()).storage(redis_cache.clone());

    // L1: in-memory with short TTL, falling back to L2
    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(10))
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    // First get: L1 miss → L2 hit → promoted to L1
    let entry = cache
        .get(&"greeting".to_string())
        .await
        .expect("get failed")
        .expect("expected a value");
    println!("1st get: {} (L1 miss → L2 hit → promoted)", entry.value());

    // Second get: L1 hit (no Redis round-trip)
    let entry = cache
        .get(&"greeting".to_string())
        .await
        .expect("get failed")
        .expect("expected a value");
    println!("2nd get: {} (L1 hit)", entry.value());

    // ── Part 2: Seatbelt resilience (retry + timeout) ───────────────────

    let redis_resilient = RedisCache::<String, String>::builder(conn)
        .key_prefix("resilient:")
        .build();

    let context = ResilienceContext::new(&clock);

    // Timeout: each attempt gets 2 seconds
    let timeout_layer = Timeout::layer("redis-timeout", &context)
        .timeout(Duration::from_secs(2))
        .timeout_error(|_| cachet::Error::from_message("redis operation timed out"));

    // Retry: retry on any error
    let retry_layer = Retry::layer("redis-retry", &context)
        .clone_input()
        .recovery_with(|res: &Result<_, _>, _| match res {
            Ok(_) => RecoveryInfo::never(),
            Err(_) => RecoveryInfo::retry(),
        });

    // Stack: retry( timeout( redis ) )
    let resilient_service = retry_layer.layer(timeout_layer.layer(redis_resilient));

    // Use resilient service as L2 via .service()
    let l2 = Cache::builder::<String, String>(clock.clone()).service(resilient_service);

    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(10))
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    // Insert through the resilience stack
    cache
        .insert(
            &"key".to_string(),
            CacheEntry::new("resilient-value".to_string()),
        )
        .await
        .expect("resilient insert failed");

    let entry = cache
        .get(&"key".to_string())
        .await
        .expect("resilient get failed")
        .expect("expected a value");
    println!("resilient get: {} (through retry + timeout stack)", entry.value());

    // Clean up
    redis_cache.clear().await.expect("cleanup failed");

    println!("\nDone! All operations succeeded.");
}
