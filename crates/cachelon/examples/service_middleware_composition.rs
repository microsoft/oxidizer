// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compose middleware with cache operations.
//!
//! Since Cache implements `Service<CacheOperation>`, you can wrap it
//! with any layered middleware (logging, metrics, retry, timeout).

use cachelon::{Cache, CacheEntry, CacheOperation, CacheResponse, CacheServiceExt};
use layered::{Layer, Service};
use tick::Clock;

/// Simple logging middleware that prints each operation.
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

#[tokio::main]
async fn main() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, String>(clock).memory().build();

    // Wrap cache with logging middleware
    let logged_cache = LoggingLayer.layer(cache);

    // Use CacheServiceExt for ergonomic methods on the wrapped service
    logged_cache
        .insert(&"key".to_string(), CacheEntry::new("value".to_string()))
        .await
        .expect("insert failed");

    let value = logged_cache.get(&"key".to_string()).await.expect("get failed");
    match value {
        Some(e) => println!("get(key): {}", e.value()),
        None => println!("get(key): not found"),
    }
}
