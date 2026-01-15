// Copyright (c) Microsoft Corporation.

//! Example demonstrating Cache as a Service node in a service hierarchy.
//!
//! This shows how to compose Cache with middleware like retry and timeout,
//! enabling resilience around cache operations.

use cachelon::{Cache, CacheEntry, CacheOperation, GetRequest, InsertRequest};
use layered::Service;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();

    // Build a simple in-memory cache
    let cache = Cache::builder::<String, String>(clock.clone()).memory().build();

    // Approach 1: Using Cache API directly
    let key1 = "user:123".to_string();
    cache.insert(&key1, CacheEntry::new("Alice".to_string())).await;

    let _entry = cache.get(&key1).await;

    // Approach 2: Using Cache as a Service
    // Cache implements Service<CacheOperation> for middleware composition
    let key2 = "user:456".to_string();
    let insert_request = CacheOperation::Insert(InsertRequest::new(key2.clone(), CacheEntry::new("Bob".to_string())));

    let _ = cache.execute(insert_request).await;

    let get_request = CacheOperation::Get(GetRequest::new(key2.clone()));
    let _ = cache.execute(get_request).await;

    // Benefits: Cache can be composed in service middleware stacks
    //   let service_stack = (
    //       Timeout::layer(...).timeout(Duration::from_secs(1)),
    //       Retry::layer(...).max_retry_attempts(3),
    //       cache,
    //   ).build();
}
