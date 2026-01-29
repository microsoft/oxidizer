// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Time-to-refresh: return stale data immediately, refresh in background.
//! Keeps cache warm and avoids latency spikes from cache misses.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use cachelon::{Cache, CacheEntry, CacheTier, FallbackPromotionPolicy, refresh::TimeToRefresh};
use tick::Clock;

#[derive(Clone)]
struct Database(Arc<AtomicU32>);

impl CacheTier<String, String> for Database {
    async fn get(&self, key: &String) -> Option<CacheEntry<String>> {
        let v = self.0.fetch_add(1, Ordering::Relaxed) + 1;
        tokio::time::sleep(Duration::from_millis(50)).await;
        Some(CacheEntry::new(format!("{key}_v{v}")))
    }
    async fn insert(&self, _: &String, _: CacheEntry<String>) {}
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let db = Database(Arc::new(AtomicU32::new(0)));

    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(10))
        .fallback(Cache::builder::<String, String>(clock).storage(db.clone()))
        .time_to_refresh(TimeToRefresh::new_tokio(Duration::from_secs(1)))
        .promotion_policy(FallbackPromotionPolicy::Always)
        .build();

    let key = "config".to_string();

    // Initial fetch (db call #1)
    let v = cache.get(&key).await;
    println!(
        "initial: {:?} (db calls: {})",
        v.map(|e| e.value().clone()),
        db.0.load(Ordering::Relaxed)
    );

    // Wait past refresh threshold
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Returns stale value immediately, triggers background refresh
    let v = cache.get(&key).await;
    println!("stale: {:?}", v.map(|e| e.value().clone()));

    // Wait for refresh to complete
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now returns refreshed value
    let v = cache.get(&key).await;
    println!(
        "refreshed: {:?} (db calls: {})",
        v.map(|e| e.value().clone()),
        db.0.load(Ordering::Relaxed)
    );
}
