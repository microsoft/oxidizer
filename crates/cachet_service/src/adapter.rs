// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapter to use Service implementations as `CacheTier` storage backends.
//!
//! This module provides bidirectional adaptation between `Service<CacheOperation>`
//! and `CacheTier`, enabling remote cache services (Redis, Memcached) to be used
//! as cache storage backends.

use std::future::ready;
use std::hash::Hash;
use std::marker::PhantomData;

use cachet_tier::{CacheEntry, CacheTier, Error, InsertOutcome, SizeError};
use layered::Service;

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
            _ => Err(Error::from_message("unexpected response type for get")),
        }
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<InsertOutcome, Error> {
        let request = CacheOperation::Insert(InsertRequest::new(key, entry));
        match self.service.execute(request).await? {
            CacheResponse::Insert(outcome) => Ok(outcome),
            _ => Err(Error::from_message("unexpected response type for insert")),
        }
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let request = CacheOperation::Invalidate(InvalidateRequest::new(key.clone()));
        match self.service.execute(request).await? {
            CacheResponse::Invalidate => Ok(()),
            _ => Err(Error::from_message("unexpected response type for invalidate")),
        }
    }

    async fn clear(&self) -> Result<(), Error> {
        match self.service.execute(CacheOperation::Clear).await? {
            CacheResponse::Clear => Ok(()),
            _ => Err(Error::from_message("unexpected response type for clear")),
        }
    }

    fn len(&self) -> impl Future<Output = Result<u64, SizeError>> + Send {
        // Service-based tiers typically don't expose length information
        ready(Err(SizeError::unsupported()))
    }
}
