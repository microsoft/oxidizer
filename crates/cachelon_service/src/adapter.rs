// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapter to use Service implementations as `CacheTier` storage backends.
//!
//! This module provides bidirectional adaptation between `Service<CacheOperation>`
//! and `CacheTier`, enabling remote cache services (Redis, Memcached) to be used
//! as cache storage backends.

use std::{hash::Hash, marker::PhantomData};

use layered::Service;

use cachelon_tier::{CacheEntry, CacheTier, Error};

use crate::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};

/// Adapter that converts a `Service<CacheOperation>` into a `CacheTier`.
///
/// This enables using service-based cache implementations (like Redis or Memcached)
/// as storage backends for `Cache`. The service can be composed with middleware
/// (retry, timeout, circuit breakers) before being wrapped by this adapter.
///
/// # Examples
///
/// ```ignore
/// // Convert any Service<CacheOperation> to a CacheTier
/// let adapter = ServiceAdapter::new(redis_service);
///
/// // Use as cache storage
/// let cache = Cache::builder(clock)
///     .storage(adapter)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct ServiceAdapter<K, V, S> {
    service: S,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, S> ServiceAdapter<K, V, S> {
    /// Creates a new `ServiceAdapter` wrapping the given service.
    ///
    /// The service must implement `Service<CacheOperation<K, V>>` with
    /// output type `Result<CacheResponse<V>, Error>`.
    #[must_use]
    pub fn new(service: S) -> Self {
        Self {
            service,
            _phantom: PhantomData,
        }
    }

    /// Consumes the adapter and returns the inner service.
    #[must_use]
    pub fn into_inner(self) -> S {
        self.service
    }

    /// Returns a reference to the inner service.
    #[must_use]
    pub fn inner(&self) -> &S {
        &self.service
    }
}

impl<K, V, S> CacheTier<K, V> for ServiceAdapter<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: Service<CacheOperation<K, V>, Out = Result<CacheResponse<V>, Error>> + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let request = CacheOperation::Get(GetRequest::new(key.clone()));
        match self.service.execute(request).await? {
            CacheResponse::Get(entry) => Ok(entry),
            _ => Ok(None),
        }
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        let request = CacheOperation::Insert(InsertRequest::new(key.clone(), entry));
        match self.service.execute(request).await? {
            CacheResponse::Insert(()) => Ok(()),
            _ => Err(Error::from_message("unexpected response type for insert")),
        }
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let request = CacheOperation::Invalidate(InvalidateRequest::new(key.clone()));
        match self.service.execute(request).await? {
            CacheResponse::Invalidate(()) => Ok(()),
            _ => Err(Error::from_message("unexpected response type for invalidate")),
        }
    }

    async fn clear(&self) -> Result<(), Error> {
        match self.service.execute(CacheOperation::Clear).await? {
            CacheResponse::Clear(()) => Ok(()),
            _ => Err(Error::from_message("unexpected response type for clear")),
        }
    }

    fn len(&self) -> Option<u64> {
        // Service-based tiers typically don't expose length information
        None
    }

    fn is_empty(&self) -> Option<bool> {
        // Service-based tiers typically don't expose empty status
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock service for testing
    #[derive(Debug, Clone)]
    struct MockService;

    impl Service<CacheOperation<String, i32>> for MockService {
        type Out = Result<CacheResponse<i32>, Error>;

        async fn execute(&self, input: CacheOperation<String, i32>) -> Self::Out {
            match input {
                CacheOperation::Get(req) => {
                    if req.key == "existing" {
                        Ok(CacheResponse::Get(Some(CacheEntry::new(42))))
                    } else {
                        Ok(CacheResponse::Get(None))
                    }
                }
                CacheOperation::Insert(_) => Ok(CacheResponse::Insert(())),
                CacheOperation::Invalidate(_) => Ok(CacheResponse::Invalidate(())),
                CacheOperation::Clear => Ok(CacheResponse::Clear(())),
            }
        }
    }

    #[tokio::test]
    async fn adapter_get_existing() {
        let adapter = ServiceAdapter::new(MockService);
        let result = adapter.get(&"existing".to_string()).await;
        assert!(result.is_ok());
        assert_eq!(*result.unwrap().unwrap().value(), 42);
    }

    #[tokio::test]
    async fn adapter_get_missing() {
        let adapter = ServiceAdapter::new(MockService);
        let result = adapter.get(&"missing".to_string()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn adapter_insert() {
        let adapter = ServiceAdapter::new(MockService);
        adapter.insert(&"key".to_string(), CacheEntry::new(100)).await.unwrap();
        // No assertion - just verify it doesn't panic
    }

    #[tokio::test]
    async fn adapter_invalidate() {
        let adapter = ServiceAdapter::new(MockService);
        adapter.invalidate(&"key".to_string()).await.unwrap();
        // No assertion - just verify it doesn't panic
    }

    #[tokio::test]
    async fn adapter_clear() {
        let adapter = ServiceAdapter::new(MockService);
        adapter.clear().await.unwrap();
        // No assertion - just verify it doesn't panic
    }

    #[test]
    fn adapter_len() {
        let adapter = ServiceAdapter::<String, i32, _>::new(MockService);
        assert_eq!(adapter.len(), None);
    }

    #[test]
    fn adapter_is_empty() {
        let adapter = ServiceAdapter::<String, i32, _>::new(MockService);
        assert_eq!(adapter.is_empty(), None);
    }

    #[test]
    fn adapter_into_inner() {
        let adapter = ServiceAdapter::<String, i32, _>::new(MockService);
        let _service = adapter.into_inner();
        // Just verify it compiles and runs
    }

    #[test]
    fn adapter_inner() {
        let adapter = ServiceAdapter::<String, i32, _>::new(MockService);
        let _service = adapter.inner();
        // Just verify it compiles and runs
    }
}
