// Copyright (c) Microsoft Corporation.

//! Example demonstrating Service as cache storage backend.
//!
//! This shows how to implement a custom cache service (e.g., Redis, Memcached)
//! and use it as a CacheTier storage backend via ServiceAdapter.

use std::{collections::HashMap, sync::Arc, time::Duration};

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
use layered::Service;
use parking_lot::RwLock;
use tick::Clock;

/// Mock remote cache service (simulates Redis, Memcached, etc.)
///
/// In a real implementation, this would connect to a remote cache server.
#[derive(Clone)]
struct RemoteCacheService {
    storage: Arc<RwLock<HashMap<String, CacheEntry<String>>>>,
}

impl RemoteCacheService {
    fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Service<CacheOperation<String, String>> for RemoteCacheService {
    type Out = Result<CacheResponse<String>, cachelon::Error>;

    async fn execute(&self, input: CacheOperation<String, String>) -> Result<CacheResponse<String>, cachelon::Error> {
        match input {
            CacheOperation::Get(GetRequest { key }) => {
                let entry = self.storage.read().get(&key).cloned();
                Ok(CacheResponse::Get(entry))
            }
            CacheOperation::Insert(InsertRequest { key, entry }) => {
                self.storage.write().insert(key, entry);
                Ok(CacheResponse::Insert(()))
            }
            CacheOperation::Invalidate(InvalidateRequest { key }) => {
                self.storage.write().remove(&key);
                Ok(CacheResponse::Invalidate(()))
            }
            CacheOperation::Clear => {
                self.storage.write().clear();
                Ok(CacheResponse::Clear(()))
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();

    // Create a remote cache service (simulates Redis, Memcached, etc.)
    let remote_service = RemoteCacheService::new();

    // Step 1: Service operates independently
    let get_request = CacheOperation::Get(GetRequest::new("key1".to_string()));
    let _ = remote_service.execute(get_request).await;

    // Step 2: ServiceAdapter converts Service → CacheTier
    // Now the service can be used as Cache storage backend
    let adapter = ServiceAdapter::new(remote_service.clone());

    let cache = Cache::builder::<String, String>(clock.clone()).storage(adapter).build();

    // Insert via Cache API (backed by remote service)
    let key = "user:789".to_string();
    cache.insert(&key, CacheEntry::new("Charlie".to_string())).await;

    let _entry = cache.get(&key).await;

    // Step 3: Builder convenience method
    // Use .from_service() for simpler construction
    let remote_service2 = RemoteCacheService::new();
    let cache2 = Cache::builder::<String, String>(clock.clone())
        .from_service(remote_service2) // Wraps service in ServiceAdapter automatically
        .ttl(Duration::from_secs(300))
        .build();

    let key2 = "user:999".to_string();
    cache2.insert(&key2, CacheEntry::new("David".to_string())).await;

    let _entry2 = cache2.get(&key2).await;

    // Benefits:
    // - Remote services (Redis, Memcached) work as CacheTier
    // - Can wrap service with CacheWrapper for telemetry/TTL
    // - Can use in FallbackCache for multi-tier caching
    // - Service middleware (retry/timeout) composes before adaptation
}
