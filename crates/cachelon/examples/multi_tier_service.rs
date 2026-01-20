// Copyright (c) Microsoft Corporation.

//! Example demonstrating multi-tier cache with service-based storage.
//!
//! This shows how to build a cache hierarchy:
//! - L1: Fast in-memory cache
//! - L2: Remote service-based cache (Redis, Memcached, etc.)
//!
//! This pattern enables:
//! - Local caching for hot data
//! - Remote caching for shared data
//! - Full cache features (telemetry, TTL, promotion) on both tiers

use std::{collections::HashMap, sync::Arc, time::Duration};

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, FallbackPromotionPolicy, GetRequest, InsertRequest, InvalidateRequest};
use layered::Service;
use parking_lot::RwLock;
use tick::Clock;

/// Mock distributed cache service (simulates Redis cluster)
#[derive(Clone)]
struct DistributedCacheService {
    storage: Arc<RwLock<HashMap<String, CacheEntry<Vec<u8>>>>>,
}

impl DistributedCacheService {
    fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Service<CacheOperation<String, Vec<u8>>> for DistributedCacheService {
    type Out = Result<CacheResponse<Vec<u8>>, cachelon::Error>;

    async fn execute(&self, input: CacheOperation<String, Vec<u8>>) -> Result<CacheResponse<Vec<u8>>, cachelon::Error> {
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

    // Build cache hierarchy:
    // - L1: In-memory (fast, local) - 60 second TTL
    // - L2: Distributed service (shared) - 600 second TTL
    // - Policy: Always promote from L2 to L1

    // L2: Remote distributed cache service (simulates Redis)
    let redis_service = DistributedCacheService::new();

    let l2_builder = Cache::builder::<String, Vec<u8>>(clock.clone())
        .service(redis_service)
        .ttl(Duration::from_secs(600));

    // L1: In-memory cache with L2 fallback
    let cache = Cache::builder::<String, Vec<u8>>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(l2_builder)
        .promotion_policy(FallbackPromotionPolicy::always())
        .build();

    // Insert goes to both L1 and L2
    let data1 = vec![1, 2, 3, 4, 5];
    let key1 = "data:1".to_string();
    cache.insert(&key1, CacheEntry::new(data1.clone())).await;

    // Get from L1 (cache hit)
    let _result = cache.get(&key1).await;

    // Insert second key
    let data2 = vec![6, 7, 8, 9, 10];
    let key2 = "data:2".to_string();
    cache.insert(&key2, CacheEntry::new(data2.clone())).await;

    // Get hits L1 immediately after insert
    let _result = cache.get(&key2).await;

    // Benefits:
    // - L1 provides fast local caching
    // - L2 service enables shared/distributed caching
    // - ServiceAdapter + CacheWrapper provides full cache features
    // - Promotion policies control L1 <-> L2 data flow
    // - Independent TTLs per tier
    // - Service middleware (retry/timeout) can wrap L2
}
