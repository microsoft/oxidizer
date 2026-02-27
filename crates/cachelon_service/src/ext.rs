// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use cachelon_tier::{CacheEntry, Error};
use layered::Service;

use crate::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};

/// Extension trait providing ergonomic cache methods for any `Service<CacheOperation>`.
///
/// This allows middleware-wrapped cache services to be used with the same
/// simple API as a direct `Cache`.
pub trait CacheServiceExt<K, V>: Sized {
    /// Retrieves a value from the cache.
    fn get(&self, key: &K) -> impl Future<Output = Result<Option<CacheEntry<V>>, Error>> + Send;
    /// Inserts a value into the cache.
    fn insert(&self, key: &K, entry: CacheEntry<V>) -> impl Future<Output = Result<(), Error>> + Send;
    /// Invalidates (removes) a value from the cache.
    fn invalidate(&self, key: &K) -> impl Future<Output = Result<(), Error>> + Send;
    /// Clears all entries from the cache.
    fn clear(&self) -> impl Future<Output = Result<(), Error>> + Send;
}

impl<K, V, S> CacheServiceExt<K, V> for S
where
    K: Clone + Send + Sync,
    V: Clone + Send + Sync,
    S: Service<CacheOperation<K, V>, Out = Result<CacheResponse<V>, Error>> + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let req = GetRequest { key: key.clone() };
        match self.execute(CacheOperation::Get(req)).await? {
            CacheResponse::Get(entry) => Ok(entry),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        let req = InsertRequest { key: key.clone(), entry };
        match self.execute(CacheOperation::Insert(req)).await? {
            CacheResponse::Insert() => Ok(()),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let req = InvalidateRequest { key: key.clone() };
        match self.execute(CacheOperation::Invalidate(req)).await? {
            CacheResponse::Invalidate() => Ok(()),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }

    async fn clear(&self) -> Result<(), Error> {
        match self.execute(CacheOperation::Clear).await? {
            CacheResponse::Clear() => Ok(()),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }
}
