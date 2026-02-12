// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Use a `Service<CacheOperation>` as cache storage via `.service()`.
//!
//! This pattern is useful for remote caches (Redis, Memcached) where
//! you want to add middleware (retry, timeout) to the underlying service.

use std::collections::HashMap;

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};
use layered::Service;
use tick::Clock;

/// Simple in-memory service (in practice, this would be a Redis client, etc.)
#[derive(Clone, Default)]
struct RemoteCache {
    data: HashMap<String, CacheEntry<String>>,
}

impl Service<CacheOperation<String, String>> for RemoteCache {
    type Out = Result<CacheResponse<String>, cachelon::Error>;

    async fn execute(&self, input: CacheOperation<String, String>) -> Self::Out {
        match input {
            CacheOperation::Get(GetRequest { key }) => Ok(CacheResponse::Get(self.data.get(&key).cloned())),
            CacheOperation::Insert(InsertRequest { key, entry }) => {
                // Note: simplified - real impl would mutate
                let _ = (key, entry);
                Ok(CacheResponse::Insert())
            }
            CacheOperation::Invalidate(InvalidateRequest { key }) => {
                let _ = key;
                Ok(CacheResponse::Invalidate())
            }
            CacheOperation::Clear => Ok(CacheResponse::Clear()),
        }
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();

    // .service() wraps the Service in ServiceAdapter, converting it to CacheTier
    let cache = Cache::builder::<String, String>(clock)
        .service(RemoteCache::default())
        .build();

    // Use cache normally - operations go through the Service
    cache
        .insert(&"key".to_string(), CacheEntry::new("value".to_string()))
        .await
        .expect("insert failed");

    let value = cache.get(&"key".to_string()).await.expect("get failed");
    match value {
        Some(e) => println!("get(key): {}", e.value()),
        None => println!("get(key): not found"),
    }
}
