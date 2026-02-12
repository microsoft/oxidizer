// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `MockCache` for testing: record operations, inject failures, pre-populate data.

use cachelon::{Cache, CacheEntry};
use cachelon_tier::testing::{CacheOp, MockCache};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let mock = MockCache::<String, i32>::new();
    let cache = Cache::builder(clock).storage(mock.clone()).build();

    // Operations are recorded
    cache.insert(&"key".to_string(), CacheEntry::new(42)).await.expect("insert failed");
    cache.get(&"key".to_string()).await.expect("get failed");

    println!("operations: {} recorded", mock.operations().len());

    // Inject failures for testing error paths
    mock.fail_when(|op| matches!(op, CacheOp::Get(_)));
    let result = cache.get(&"key".to_string()).await;
    match result {
        Ok(_) => println!("after fail_when: unexpected success"),
        Err(e) => println!("after fail_when: {e}"),
    }

    // Clear failures
    mock.clear_failures();
    let result = cache.get(&"key".to_string()).await.expect("get failed");
    match result {
        Some(e) => println!("after clear_failures: {}", e.value()),
        None => println!("after clear_failures: not found"),
    }
}
