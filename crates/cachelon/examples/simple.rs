// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic cache operations: get, insert, invalidate.

use std::time::Duration;

use cachelon::Cache;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .build();

    let key = "user:1".to_string();

    // Insert and retrieve
    cache.insert(&key, "Alice".to_string().into()).await;
    let value = cache.get(&key).await;
    println!("get({key}): {:?}", value.map(|e| e.value().clone()));

    // Invalidate
    cache.invalidate(&key).await;
    let value = cache.get(&key).await;
    println!("after invalidate: {:?}", value.map(|e| e.value().clone()));
}
