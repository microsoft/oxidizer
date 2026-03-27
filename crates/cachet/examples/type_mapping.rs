// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Type-mapping example: composite key to simple key.
//!
//! L1 uses a rich composite key (`CacheKey`) for lookups.
//! L2 uses a simple `String` key derived from the composite.
//! The transform boundary converts between them.

use std::time::Duration;

use cachet::{Cache, CacheEntry, MockCache, TransformCodec, TransformEncoder};
use tick::Clock;

/// A composite cache key with tenant and resource ID.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct CacheKey {
    tenant: String,
    resource_id: u64,
}

impl CacheKey {
    fn new(tenant: impl Into<String>, resource_id: u64) -> Self {
        Self {
            tenant: tenant.into(),
            resource_id,
        }
    }

    /// Flattens the composite key into a simple string for L2 storage.
    fn to_flat_key(&self) -> String {
        format!("{}:{}", self.tenant, self.resource_id)
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // L2: uses simple String keys and String values.
    let l2 = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .ttl(Duration::from_secs(300));

    // Build the cache with a type-mapping boundary:
    //   L1 uses CacheKey → String
    //   L2 uses String → String (after transform)
    let cache = Cache::builder::<CacheKey, String>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .transform(
            // Key: CacheKey → String (flatten composite key)
            TransformEncoder::infallible(|k: &CacheKey| k.to_flat_key()),
            // Value: String ↔ String (identity — no value mapping needed)
            TransformCodec::new(
                |v: &String| Ok::<_, std::convert::Infallible>(v.clone()),
                |v: &String| Ok::<_, std::convert::Infallible>(v.clone()),
            ),
        )
        .fallback(l2)
        .build();

    let key = CacheKey::new("acme", 42);

    // Insert with the composite key
    cache
        .insert(key.clone(), CacheEntry::new("widget-data".to_string()))
        .await
        .expect("insert failed");

    // Retrieve — L1 uses CacheKey, L2 uses "acme:42"
    let result = cache.get(&key).await.expect("get failed");
    match result {
        Some(entry) => println!("got: {}", entry.value()),
        None => println!("not found"),
    }

    println!("done");
}
