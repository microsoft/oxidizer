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

use anyspawn::Spawner;
use cachelon::{Cache, CacheEntry, CacheTier, Error, FallbackPromotionPolicy, refresh::TimeToRefresh};
use tick::Clock;

#[derive(Clone)]
struct Database {
    calls: Arc<AtomicU32>,
    clock: Clock,
}

impl CacheTier<String, String> for Database {
    async fn get(&self, key: &String) -> Result<Option<CacheEntry<String>>, Error> {
        let v = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        self.clock.delay(Duration::from_millis(50)).await;
        Ok(Some(CacheEntry::new(format!("{key}_v{v}"))))
    }

    async fn insert(&self, _: &String, _: CacheEntry<String>) -> Result<(), Error> {
        Ok(())
    }

    async fn invalidate(&self, _: &String) -> Result<(), Error> {
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let db = Database {
        calls: Arc::new(AtomicU32::new(0)),
        clock: clock.clone(),
    };

    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(10))
        .fallback(Cache::builder::<String, String>(clock.clone()).storage(db.clone()))
        .time_to_refresh(TimeToRefresh::new(Duration::from_secs(1), Spawner::new_tokio()))
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    let key = "config".to_string();

    // Initial fetch (db call #1)
    let v = cache.get(&key).await.expect("get failed");
    println!(
        "initial: {:?} (db calls: {})",
        v.map(|e| e.value().clone()),
        db.calls.load(Ordering::Relaxed)
    );

    // Wait past refresh threshold
    clock.delay(Duration::from_millis(1500)).await;

    // Returns stale value immediately, triggers background refresh
    let v = cache.get(&key).await.expect("get failed");
    println!("stale: {:?}", v.map(|e| e.value().clone()));

    // Wait for refresh to complete
    clock.delay(Duration::from_millis(100)).await;

    // Now returns refreshed value
    let v = cache.get(&key).await.expect("get failed");
    println!(
        "refreshed: {:?} (db calls: {})",
        v.map(|e| e.value().clone()),
        db.calls.load(Ordering::Relaxed)
    );
}
