// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache implements Service<CacheOperation>, enabling middleware composition.

use cachelon::{Cache, CacheEntry, CacheOperation, GetRequest, InsertRequest};
use layered::Service;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, String>(clock).memory().build();

    // Use as Service<CacheOperation> for middleware composition
    let insert = CacheOperation::Insert(InsertRequest::new("key".to_string(), CacheEntry::new("value".to_string())));
    let _ = cache.execute(insert).await;

    let get = CacheOperation::Get(GetRequest::new("key".to_string()));
    let response = cache.execute(get).await;
    println!("response: {response:?}");

    // This enables wrapping with retry, timeout, etc:
    // let stack = (Retry::layer(...), Timeout::layer(...), cache);
}
