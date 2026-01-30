// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier cache with conditional promotion policies.
//! Example: only promote "not found" results to avoid repeated backend queries.

use std::{sync::Arc, time::Duration};

use cachelon::{Cache, CacheEntry, CacheTier, FallbackPromotionPolicy};
use parking_lot::Mutex;
use tick::Clock;

#[derive(Clone, Debug, PartialEq)]
enum UserData {
    Found(String),
    NotFound,
}

#[derive(Debug)]
struct Database(Mutex<u32>);

impl CacheTier<String, UserData> for Arc<Database> {
    async fn get(&self, key: &String) -> Option<CacheEntry<UserData>> {
        *self.0.lock() += 1;
        let data = match key.as_str() {
            "user:1" => UserData::Found("Alice".to_string()),
            _ => UserData::NotFound,
        };
        Some(CacheEntry::new(data))
    }
    async fn insert(&self, _: &String, _: CacheEntry<UserData>) {}
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let db = Arc::new(Database(Mutex::new(0)));

    // L2: database
    let l2 = Cache::builder::<String, UserData>(clock.clone()).storage(Arc::clone(&db));

    // L1: only promote NotFound (negative cache)
    let cache = Cache::builder::<String, UserData>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::when_boxed(|e: &CacheEntry<UserData>| {
            matches!(e.value(), UserData::NotFound)
        }))
        .build();

    // user:1 exists - NOT cached (policy rejects Found)
    let v = cache.get(&"user:1".to_string()).await;
    println!("user:1: {:?}", v.map(|e| e.value().clone()));

    // user:2 not found - cached (policy accepts NotFound)
    let v = cache.get(&"user:2".to_string()).await;
    println!("user:2: {:?}", v.map(|e| e.value().clone()));

    println!("db calls after first round: {}", *db.0.lock());

    // Second round
    cache.get(&"user:1".to_string()).await; // db call (not cached)
    cache.get(&"user:2".to_string()).await; // cache hit (was promoted)

    println!("db calls after second round: {}", *db.0.lock());
}
