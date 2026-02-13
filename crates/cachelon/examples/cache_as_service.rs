// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache implements `Service<CacheOperation>`, enabling middleware composition.
//!
//! When you wrap `Cache` in middleware (retry, timeout, etc.), the result is no
//! longer a `Cache` type, so you lose access to native methods like `.get()`.
//!
//! `CacheServiceExt` provides those ergonomic methods for any `Service<CacheOperation>`.

use cachelon::{Cache, CacheEntry, CacheServiceExt};
use layered::Layer;
use seatbelt::{RecoveryInfo, ResilienceContext, retry::Retry};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    let cache = Cache::builder::<String, String>(clock).memory().build();

    // Wrap Cache in retry middleware - result is no longer a `Cache`
    let retry_layer = Retry::layer("cache-retry", &context)
        .clone_input()
        .recovery_with(|res: &Result<_, _>, _| match res {
            Ok(_) => RecoveryInfo::never(),
            Err(_) => RecoveryInfo::retry(),
        });
    let cache_with_retry = retry_layer.layer(cache);

    // CacheServiceExt provides .get(), .insert() on any Service<CacheOperation>
    cache_with_retry
        .insert(&"key".to_string(), CacheEntry::new("value".to_string()))
        .await
        .expect("insert failed");

    let entry = cache_with_retry.get(&"key".to_string()).await.expect("get failed");
    match entry {
        Some(e) => println!("get(key): {}", e.value()),
        None => println!("get(key): not found"),
    }
}
