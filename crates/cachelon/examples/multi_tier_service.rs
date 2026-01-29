// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier cache: L1 in-memory + L2 service-based (e.g., Redis).

use std::{collections::HashMap, sync::Arc, time::Duration};

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, FallbackPromotionPolicy, GetRequest, InsertRequest, InvalidateRequest};
use layered::Service;
use parking_lot::RwLock;
use tick::Clock;

#[derive(Clone)]
struct RedisService(Arc<RwLock<HashMap<String, CacheEntry<String>>>>);

impl Service<CacheOperation<String, String>> for RedisService {
    type Out = Result<CacheResponse<String>, cachelon::Error>;

    async fn execute(&self, input: CacheOperation<String, String>) -> Self::Out {
        match input {
            CacheOperation::Get(GetRequest { key }) => Ok(CacheResponse::Get(self.0.read().get(&key).cloned())),
            CacheOperation::Insert(InsertRequest { key, entry }) => {
                self.0.write().insert(key, entry);
                Ok(CacheResponse::Insert(()))
            }
            CacheOperation::Invalidate(InvalidateRequest { key }) => {
                self.0.write().remove(&key);
                Ok(CacheResponse::Invalidate(()))
            }
            CacheOperation::Clear => {
                self.0.write().clear();
                Ok(CacheResponse::Clear(()))
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();

    // L2: Redis (longer TTL, shared)
    let redis = RedisService(Arc::new(RwLock::new(HashMap::new())));
    let l2 = Cache::builder::<String, String>(clock.clone())
        .service(redis)
        .ttl(Duration::from_secs(600));

    // L1: in-memory (shorter TTL, local)
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(l2)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    cache.insert(&"key".to_string(), CacheEntry::new("value".to_string())).await;
    let v = cache.get(&"key".to_string()).await;
    println!("get(key): {:?}", v.map(|e| e.value().clone()));
}
