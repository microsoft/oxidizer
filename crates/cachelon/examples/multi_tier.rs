// Copyright (c) Microsoft Corporation.

//! Multi-Tier Cache with Promotion Filtering Example
//!
//! Demonstrates how to use `FallbackPromotionPolicy` to control which values
//! get promoted to different cache tiers:
//!
//! - **Negative cache**: Caches "not found" results (empty `Option<T>`) to avoid
//!   repeatedly querying for non-existent keys
//! - **Invalid/Error cache**: Caches error responses separately from successful ones
//!
//! Architecture:
//! - L1 (Invalid Cache): Only stores values marked as "invalid" (e.g., error responses)
//! - L2 (Negative Cache): Only stores `None` values (cache misses from downstream)
//! - L3 (Persistent Cache): The authoritative data source
//!
//! This pattern is useful when you want to:
//! 1. Avoid hammering a slow backend with repeated queries for missing keys
//! 2. Cache error responses separately with shorter TTLs
//! 3. Keep "good" data flowing through without polluting error caches

use std::{sync::Arc, time::Duration};

use cachelon::{Cache, CacheEntry, CacheTelemetry, CacheTier, Error, FallbackPromotionPolicy};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use parking_lot::Mutex;
use tick::Clock;

fn setup_telemetry(clock: Clock) -> CacheTelemetry {
    let logger_provider = SdkLoggerProvider::builder().build();
    let meter_provider = SdkMeterProvider::builder().build();

    CacheTelemetry::new(logger_provider, &meter_provider, clock)
}

/// Represents a response that might be valid, invalid (error), or missing.
#[derive(Clone, Debug, PartialEq)]
enum UserData {
    /// Valid user data
    Found(String),
    /// User exists but data is invalid/corrupted
    Invalid(String),
    /// User not found
    NotFound,
}

/// A mock "database" that returns different types of responses.
/// This simulates the slowest tier (L3) that would be a real database or API.
#[derive(Debug)]
struct MockDatabase {
    call_count: Mutex<u32>,
}

impl MockDatabase {
    fn new() -> Self {
        Self { call_count: Mutex::new(0) }
    }

    fn call_count(&self) -> u32 {
        *self.call_count.lock()
    }
}

impl CacheTier<String, UserData> for Arc<MockDatabase> {
    async fn get(&self, key: &String) -> Option<CacheEntry<UserData>> {
        *self.call_count.lock() += 1;

        // Simulate different responses based on key
        let data = match key.as_str() {
            "user:1" => UserData::Found("Alice".to_string()),
            "user:2" => UserData::Invalid("corrupted data".to_string()),
            "user:3" => UserData::NotFound,
            _ => return None,
        };

        Some(CacheEntry::new(data))
    }

    async fn try_get(&self, key: &String) -> Result<Option<CacheEntry<UserData>>, Error> {
        Ok(self.get(key).await)
    }

    async fn insert(&self, _key: &String, _value: CacheEntry<UserData>) {
        // Database doesn't support insert in this example
    }

    async fn try_insert(&self, _key: &String, _value: CacheEntry<UserData>) -> Result<(), Error> {
        Ok(())
    }

    async fn invalidate(&self, _key: &String) {}

    async fn try_invalidate(&self, _key: &String) -> Result<(), Error> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let cachelon_telemetry = setup_telemetry(clock.clone());

    // Create a mock database as our "source of truth"
    let database = Arc::new(MockDatabase::new());

    // Build a three-tier cache hierarchy with promotion filtering
    //
    // Data flows: Query -> L1 -> L2 -> L3 (database)
    // Promotion flows back: L3 -> (maybe L2) -> (maybe L1) -> Response
    //
    // Each FallbackBuilder controls its own promotion policy:
    // - L2 -> L3: Only promote NotFound values (negative cache)
    // - L1 -> L2: Only promote Invalid values (error cache)

    // L3: Database tier
    let l3 = Cache::builder::<String, UserData>(clock.clone()).storage(Arc::clone(&database));

    // L2: Negative cache - only promotes NotFound values from L3
    let l2 = Cache::builder::<String, UserData>(clock.clone())
        .memory()
        .with_fallback(l3)
        .promotion_policy(FallbackPromotionPolicy::when_boxed(|entry: &CacheEntry<UserData>| {
            matches!(entry.value(), UserData::NotFound)
        }));

    // L1: Invalid cache - only promotes Invalid values from L2
    let l1_cache = Cache::builder::<String, UserData>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(60))
        .with_fallback(l2)
        .telemetry(cachelon_telemetry, "L1-invalid-cache")
        .promotion_policy(FallbackPromotionPolicy::when_boxed(|entry: &CacheEntry<UserData>| {
            matches!(entry.value(), UserData::Invalid(_))
        }))
        .build();

    // First round of queries - all miss cache, hit database
    // user:1 returns valid data - NOT promoted to L1 or L2
    let _value = l1_cache.get(&"user:1".to_string()).await;

    // user:2 returns invalid data - promoted to L1 only
    let _value = l1_cache.get(&"user:2".to_string()).await;

    // user:3 returns not found - promoted to L2 only
    let _value = l1_cache.get(&"user:3".to_string()).await;

    let first_round_db_calls = database.call_count();

    // Second round of queries - should hit appropriate cache tiers
    // user:1 (valid) - cache MISS (L1, L2), hits database again
    let _ = l1_cache.get(&"user:1".to_string()).await;

    // user:2 (invalid) - cache HIT on L1 (no database call)
    let _ = l1_cache.get(&"user:2".to_string()).await;

    // user:3 (not found) - cache MISS on L1, HIT on L2 (no database call)
    let _ = l1_cache.get(&"user:3".to_string()).await;

    let second_round_db_calls = database.call_count() - first_round_db_calls;
    assert_eq!(second_round_db_calls, 1); // Only user:1 hits database

    // Demonstrate len() and clear()
    let _l1_entries = l1_cache.len();

    // Clear the cache
    l1_cache.clear().await;

    let _l1_entries_after_clear = l1_cache.len();

    // Query again to show it hits the database
    let _ = l1_cache.get(&"user:2".to_string()).await;
    // Total database calls after clear should be higher
}
