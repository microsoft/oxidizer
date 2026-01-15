// Copyright (c) Microsoft Corporation.

//! Stampede Protection Example
//!
//! Demonstrates how `get_coalesced` prevents the "thundering herd" problem
//! where many concurrent cache misses for the same key overwhelm the backend.
//!
//! When multiple tasks request the same uncached key simultaneously:
//! - Without protection: Each task calls the backend (N calls for N tasks)
//! - With `get_coalesced`: Only one task calls the backend, others wait and share the result

use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use cachelon::{Cache, CacheEntry, CacheTier};
use tick::Clock;

/// A slow backend that counts how many times it's called.
/// Wrapped in a newtype to satisfy orphan rules for trait implementation.
#[derive(Debug, Clone)]
struct SlowBackend {
    call_count: Arc<AtomicU32>,
    latency: Duration,
}

impl SlowBackend {
    fn new(latency: Duration) -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            latency,
        }
    }

    fn call_count(&self) -> u32 {
        self.call_count.load(Ordering::Relaxed)
    }
}

impl CacheTier<String, String> for SlowBackend {
    async fn get(&self, key: &String) -> Option<CacheEntry<String>> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(self.latency).await;
        Some(CacheEntry::new(format!("value_for_{key}")))
    }

    async fn insert(&self, _key: &String, _entry: CacheEntry<String>) {
        // Backend is read-only in this example
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let num_concurrent = 10;
    let backend_latency = Duration::from_millis(100);

    // Scenario 1: Without stampede protection (regular get)
    // Each task calls the backend independently
    let backend = SlowBackend::new(backend_latency);
    let cache = Arc::new(Cache::builder::<String, String>(clock.clone()).storage(backend.clone()).build());

    let key = "contested_key".to_string();
    let start = std::time::Instant::now();

    // Spawn N concurrent tasks all requesting the same key
    let mut handles = Vec::new();
    for _ in 0..num_concurrent {
        let cache = Arc::clone(&cache);
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            // Each task independently calls the backend
            let _result = cache.get(&key).await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let _elapsed = start.elapsed();
    let _calls = backend.call_count();
    // Without protection: N backend calls (one per task)

    // Scenario 2: With stampede protection (get_coalesced)
    // Only one task calls the backend, others share the result
    let backend = SlowBackend::new(backend_latency);
    let cache = Arc::new(Cache::builder::<String, String>(clock.clone()).storage(backend.clone()).build());

    let key = "contested_key".to_string();
    let start = std::time::Instant::now();

    // Spawn N concurrent tasks all requesting the same key
    let mut handles = Vec::new();
    for _ in 0..num_concurrent {
        let cache = Arc::clone(&cache);
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            // Only one task calls the backend, others wait and share the result
            let _result = cache.get_coalesced(&key).await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let _elapsed = start.elapsed();
    let _calls = backend.call_count();
    // With protection: 1 backend call (shared across all tasks)
}
