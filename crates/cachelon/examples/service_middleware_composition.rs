// Copyright (c) Microsoft Corporation.

//! Example demonstrating service middleware composition with cache.
//!
//! This shows how to compose service middleware (like logging) with both:
//! 1. Cache as a service (middleware wraps cache operations)
//! 2. Service as storage (middleware wraps remote cache before adaptation)

use std::{collections::HashMap, sync::Arc};

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
use layered::{Layer, Service};
use parking_lot::RwLock;
use tick::Clock;

/// Simple logging middleware for demonstration
#[derive(Clone)]
struct LoggingMiddleware<S> {
    inner: S,
    prefix: &'static str,
}

struct LoggingLayer {
    prefix: &'static str,
}

impl LoggingLayer {
    fn new(prefix: &'static str) -> Self {
        Self { prefix }
    }
}

impl<S> Layer<S> for LoggingLayer {
    type Service = LoggingMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LoggingMiddleware {
            inner,
            prefix: self.prefix,
        }
    }
}

impl<S> Service<CacheOperation<String, String>> for LoggingMiddleware<S>
where
    S: Service<CacheOperation<String, String>, Out = Result<CacheResponse<String>, cachelon::Error>> + Send + Sync,
{
    type Out = Result<CacheResponse<String>, cachelon::Error>;

    async fn execute(&self, input: CacheOperation<String, String>) -> Result<CacheResponse<String>, cachelon::Error> {
        let op_name = match &input {
            CacheOperation::Get(_) => "GET",
            CacheOperation::Insert(_) => "INSERT",
            CacheOperation::Invalidate(_) => "INVALIDATE",
            CacheOperation::Clear => "CLEAR",
        };

        // Execute the inner service operation
        let _prefix = self.prefix;
        let _op_name = op_name;
        self.inner.execute(input).await
    }
}

/// Mock remote cache service
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

    // Pattern 1: Middleware wrapping Cache
    // Cache implements Service, so middleware can wrap it
    let cache1 = Cache::builder::<String, String>(clock.clone()).memory().build();

    // Wrap cache with logging middleware
    let logging_layer = LoggingLayer::new("CACHE-SERVICE");
    let logged_cache = logging_layer.layer(cache1);

    // Execute cache operations through logging middleware
    let insert_req = CacheOperation::Insert(InsertRequest::new("key1".to_string(), CacheEntry::new("value1".to_string())));
    let _ = logged_cache.execute(insert_req).await;

    let get_req = CacheOperation::Get(GetRequest::new("key1".to_string()));
    let _ = logged_cache.execute(get_req).await;

    // Pattern 2: Middleware wrapping service before adaptation
    // Wrap remote services with middleware, THEN adapt to CacheTier
    let remote_service = RemoteCacheService::new();

    // Wrap remote service with logging BEFORE converting to CacheTier
    let logging_layer2 = LoggingLayer::new("REMOTE-SERVICE");
    let logged_remote = logging_layer2.layer(remote_service);

    // Now adapt the logged service to CacheTier
    let adapter = ServiceAdapter::new(logged_remote);
    let cache2 = Cache::builder::<String, String>(clock.clone()).storage(adapter).build();

    // Use cache backed by logged remote service
    cache2.insert(&"key2".to_string(), CacheEntry::new("value2".to_string())).await;

    let _result = cache2.get(&"key2".to_string()).await;

    // Benefits:
    // - Middleware can wrap Cache operations (pattern 1)
    // - Middleware can wrap remote services before adaptation (pattern 2)
    // - Standard service middleware (logging, metrics, tracing) works
    // - Can compose multiple middleware layers
    // - Resilience middleware (retry, timeout) fits naturally
}
