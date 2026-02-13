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
    cache.insert(&key, "Alice".to_string().into()).await.expect("insert failed");
    let value = cache.get(&key).await.expect("get failed");
    match value {
        Some(e) => println!("get({key}): {}", e.value()),
        None => println!("get({key}): not found"),
    }

    // Invalidate
    cache.invalidate(&key).await.expect("invalidate failed");
    let value = cache.get(&key).await.expect("get failed");
    match value {
        Some(e) => println!("after invalidate: {}", e.value()),
        None => println!("after invalidate: not found"),
    }
}
