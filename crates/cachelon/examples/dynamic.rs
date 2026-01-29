// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `DynamicCache` erases complex nested storage types via dynamic dispatch.
//! Trade-off: ~60-100ns overhead per operation, negligible for I/O-bound caches.

use cachelon::{Cache, CacheEntry, DynamicCache, DynamicCacheExt};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Build a multi-tier cache (complex nested type)
    let l2 = Cache::builder::<String, String>(clock.clone()).memory();
    let cache = Cache::builder::<String, String>(clock.clone()).memory().fallback(l2).build();

    // Convert to DynamicCache for simple type signature
    let dynamic: DynamicCache<String, String> = cache.into_inner().into_dynamic();
    println!("type: {}", std::any::type_name_of_val(&dynamic));

    // Wrap in Cache for full API
    let cache = Cache::builder::<String, String>(clock).storage(dynamic).build();

    cache.insert(&"key".to_string(), CacheEntry::new("value".to_string())).await;
    let value = cache.get(&"key".to_string()).await;
    println!("get(key): {:?}", value.map(|e| e.value().clone()));
}
