// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Use a Service (e.g., Redis client) as cache storage via `ServiceAdapter`.

use std::{collections::HashMap, sync::Arc};

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};
use layered::Service;
use parking_lot::RwLock;
use tick::Clock;

/// Mock remote cache service (simulates Redis, Memcached, etc.)
#[derive(Clone)]
struct RemoteCacheService(Arc<RwLock<HashMap<String, CacheEntry<String>>>>);

impl Service<CacheOperation<String, String>> for RemoteCacheService {
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
    let service = RemoteCacheService(Arc::new(RwLock::new(HashMap::new())));

    // .service() wraps the Service in ServiceAdapter, converting it to CacheTier
    let cache = Cache::builder::<String, String>(clock).service(service).build();

    cache.insert(&"key".to_string(), CacheEntry::new("value".to_string())).await.expect("insert failed");
    let value = cache.get(&"key".to_string()).await.expect("get failed");
    println!("get(key): {:?}", value.map(|e| e.value().clone()));
}
