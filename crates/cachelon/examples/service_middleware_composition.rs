// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compose middleware with cache operations.
//! Pattern 1: Wrap Cache (since it implements Service)
//! Pattern 2: Wrap remote service before adapting to `CacheTier`

use std::{collections::HashMap, sync::Arc};

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
use layered::{Layer, Service};
use parking_lot::RwLock;
use tick::Clock;

// Simple logging middleware
struct LoggingLayer;

impl<S> Layer<S> for LoggingLayer {
    type Service = LoggingMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        LoggingMiddleware(inner)
    }
}

struct LoggingMiddleware<S>(S);

impl<S> Service<CacheOperation<String, String>> for LoggingMiddleware<S>
where
    S: Service<CacheOperation<String, String>, Out = Result<CacheResponse<String>, cachelon::Error>> + Send + Sync,
{
    type Out = Result<CacheResponse<String>, cachelon::Error>;

    async fn execute(&self, input: CacheOperation<String, String>) -> Self::Out {
        let op = match &input {
            CacheOperation::Get(_) => "GET",
            CacheOperation::Insert(_) => "INSERT",
            CacheOperation::Invalidate(_) => "INVALIDATE",
            CacheOperation::Clear => "CLEAR",
        };
        println!("[LOG] {op}");
        self.0.execute(input).await
    }
}

#[derive(Clone)]
struct RemoteCache(Arc<RwLock<HashMap<String, CacheEntry<String>>>>);

impl Service<CacheOperation<String, String>> for RemoteCache {
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

    // Pattern: Wrap remote service with logging, then adapt to CacheTier
    let remote = RemoteCache(Arc::new(RwLock::new(HashMap::new())));
    let logged = LoggingLayer.layer(remote);
    let adapter = ServiceAdapter::new(logged);

    let cache = Cache::builder::<String, String>(clock).storage(adapter).build();

    cache.insert(&"key".to_string(), CacheEntry::new("value".to_string())).await.expect("insert failed");
    let _ = cache.get(&"key".to_string()).await.expect("get failed");
}
