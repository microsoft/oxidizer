// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization boundary example.
//!
//! Demonstrates `.serialize()` to add a postcard serialization boundary between
//! a typed L1 cache (`String, String`) and a byte-oriented L2 cache (`BytesView, BytesView`).
//! Keys and values are automatically serialized/deserialized when crossing the boundary.

use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // L2: byte-oriented cache (simulating a remote store like Redis)
    let l2 = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();

    // L1: typed cache with serialization boundary to L2
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .serialize()
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    let key = "greeting".to_string();

    // Insert a typed value — it's serialized to BytesView before reaching L2.
    cache
        .insert(key.clone(), CacheEntry::new("Hello, world!".to_string()))
        .await
        .expect("insert failed");

    // Get returns a typed value — deserialized from BytesView if fetched from L2.
    let value = cache.get(&key).await.expect("get failed");
    match value {
        Some(e) => println!("get({key}): {}", e.value()),
        None => println!("get({key}): not found"),
    }
}
