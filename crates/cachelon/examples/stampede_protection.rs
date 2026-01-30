// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stampede protection prevents multiple concurrent requests for the same key
//! from all hitting the backend. Only one request fetches; others wait and share the result.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use cachelon::{Cache, CacheEntry, CacheTier, Error};
use tick::Clock;

#[derive(Debug, Clone)]
struct SlowBackend(Arc<AtomicU32>);

impl CacheTier<String, String> for SlowBackend {
    async fn get(&self, key: &String) -> Result<Option<CacheEntry<String>>, Error> {
        self.0.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(Some(CacheEntry::new(format!("value_for_{key}"))))
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
    let key = "contested_key".to_string();

    // Without stampede protection: N concurrent requests = N backend calls
    let backend = SlowBackend(Arc::new(AtomicU32::new(0)));
    let cache = Arc::new(Cache::builder::<String, String>(clock.clone()).storage(backend.clone()).build());

    let mut handles = Vec::new();
    for _ in 0..10 {
        let cache = Arc::clone(&cache);
        let key = key.clone();
        handles.push(tokio::spawn(async move { cache.get(&key).await }));
    }
    for h in handles {
        let _ = h.await.expect("task panicked");
    }
    println!("without protection: {} backend calls", backend.0.load(Ordering::Relaxed));

    // With stampede protection: N concurrent requests = 1 backend call
    let backend = SlowBackend(Arc::new(AtomicU32::new(0)));
    let cache = Arc::new(
        Cache::builder::<String, String>(clock)
            .storage(backend.clone())
            .stampede_protection()
            .build(),
    );

    let mut handles = Vec::new();
    for _ in 0..10 {
        let cache = Arc::clone(&cache);
        let key = key.clone();
        handles.push(tokio::spawn(async move { cache.get(&key).await }));
    }
    for h in handles {
        let _ = h.await.expect("task panicked");
    }
    println!("with protection: {} backend call(s)", backend.0.load(Ordering::Relaxed));
}
