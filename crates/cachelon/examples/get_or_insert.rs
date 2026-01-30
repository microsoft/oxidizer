// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `get_or_insert`: fetch from cache, or compute and cache on miss.

use std::time::Duration;

use cachelon::Cache;
use tick::Clock;

async fn fetch_from_db(id: &str) -> String {
    tokio::time::sleep(Duration::from_millis(10)).await; // simulate latency
    format!("User<{id}>")
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let cache = Cache::builder::<String, String>(clock).memory().build();

    let key = "user:1".to_string();

    // First call: cache miss, calls fetch_from_db
    let entry = cache.get_or_insert(&key, || fetch_from_db("1")).await.expect("get_or_insert failed");
    println!("first call: {}", entry.value());

    // Second call: cache hit, no fetch
    let entry = cache.get_or_insert(&key, || fetch_from_db("1")).await.expect("get_or_insert failed");
    println!("second call: {}", entry.value());
}
