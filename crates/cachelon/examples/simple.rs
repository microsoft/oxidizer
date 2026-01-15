// Copyright (c) Microsoft Corporation.

//! Simple Cache Example
//!
//! Demonstrates basic cache operations: get, insert, invalidate, contains.

use std::time::Duration;

use cachelon::Cache;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Build a simple in-memory cache with a 5-second TTL
    let cache = Cache::builder::<String, String>(clock).memory().ttl(Duration::from_secs(5)).build();

    // Insert a value
    let key = "user:1".to_string();
    cache.insert(&key, "Alice".to_string().into()).await;

    // Check if key exists (returns true)
    let _exists = cache.contains(&key).await;

    // Retrieve the value (returns Some(CacheEntry))
    let _value = cache.get(&key).await;

    // Invalidate the key
    cache.invalidate(&key).await;

    // Verify it's gone (returns false)
    let _exists_after = cache.contains(&key).await;

    // Attempt to get a non-existent key (returns None)
    let missing_key = "user:2".to_string();
    let _missing = cache.get(&missing_key).await;
}
