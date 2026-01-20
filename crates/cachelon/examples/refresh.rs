// Copyright (c) Microsoft Corporation.

//! Time-to-Refresh Example
//!
//! Demonstrates background refresh of cache entries before they expire.
//!
//! When a cached value is older than the `time_to_refresh` duration but still valid,
//! the cache returns the stale value immediately while spawning a background task
//! to fetch a fresh value from the fallback tier. This keeps the cache warm and
//! avoids latency spikes from cache misses.
//!
//! This is useful for:
//! - High-traffic keys that should never experience a cache miss
//! - Reducing p99 latency by pre-emptively refreshing before expiration
//! - Keeping data fresh without blocking the caller

use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use cachelon::{Cache, CacheEntry, CacheTier, FallbackPromotionPolicy, refresh::TimeToRefresh};
use tick::Clock;

/// A simulated database that tracks call counts and returns incrementing values.
#[derive(Debug, Clone)]
struct MockDatabase {
    call_count: Arc<AtomicU32>,
}

impl MockDatabase {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    fn call_count(&self) -> u32 {
        self.call_count.load(Ordering::Relaxed)
    }
}

impl CacheTier<String, String> for MockDatabase {
    async fn get(&self, key: &String) -> Option<CacheEntry<String>> {
        let count = self.call_count.fetch_add(1, Ordering::Relaxed);

        // Simulate database latency
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Return a value that includes the call count so we can see refreshes
        Some(CacheEntry::new(format!("value_v{}_for_{}", count + 1, key)))
    }

    async fn insert(&self, _key: &String, _entry: CacheEntry<String>) {
        // Database is read-only in this example
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    let database = MockDatabase::new();

    // Build a cache with time-to-refresh enabled
    // - Primary: in-memory cache with 10 second TTL
    // - Fallback: mock database
    // - Refresh: after 2 seconds, trigger background refresh
    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(10))
        .fallback(Cache::builder::<String, String>(clock.clone()).storage(database.clone()))
        .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(2), clock.clone()))
        .promotion_policy(FallbackPromotionPolicy::Always)
        .build();

    let key = "config:app".to_string();

    // Initial fetch - cache miss, goes to database
    let _value = cache.get(&key).await;
    assert_eq!(database.call_count(), 1);

    // Immediate second fetch - cache hit, no database call
    let _value = cache.get(&key).await;
    assert_eq!(database.call_count(), 1);

    // Wait for time-to-refresh threshold (2 seconds)
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // This fetch returns stale value immediately, triggers background refresh
    let _value = cache.get(&key).await;
    // Database call count may be 1 or 2 depending on refresh timing

    // Give background refresh time to complete
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Next fetch should return the refreshed value
    let _value = cache.get(&key).await;
    // Total database calls: 2 (initial + background refresh)
}
