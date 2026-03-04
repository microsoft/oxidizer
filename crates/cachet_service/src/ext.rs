// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use cachet_tier::{CacheEntry, Error};
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

#[cfg(test)]
mod tests {
    use super::*;

    // A correct service that returns expected response types
    #[derive(Debug)]
    struct CorrectService;

    impl Service<CacheOperation<String, i32>> for CorrectService {
        type Out = Result<CacheResponse<i32>, Error>;

        async fn execute(&self, input: CacheOperation<String, i32>) -> Self::Out {
            match input {
                CacheOperation::Get(_) => Ok(CacheResponse::Get(Some(CacheEntry::new(42)))),
                CacheOperation::Insert(_) => Ok(CacheResponse::Insert()),
                CacheOperation::Invalidate(_) => Ok(CacheResponse::Invalidate()),
                CacheOperation::Clear => Ok(CacheResponse::Clear()),
            }
        }
    }

    // Service that returns wrong response types
    #[derive(Debug)]
    struct WrongResponseService;

    impl Service<CacheOperation<String, i32>> for WrongResponseService {
        type Out = Result<CacheResponse<i32>, Error>;

        async fn execute(&self, input: CacheOperation<String, i32>) -> Self::Out {
            match input {
                CacheOperation::Insert(_) => Ok(CacheResponse::Clear()),
                CacheOperation::Get(_) | CacheOperation::Invalidate(_) => Ok(CacheResponse::Insert()),
                CacheOperation::Clear => Ok(CacheResponse::Get(None)),
            }
        }
    }

    #[tokio::test]
    async fn ext_get_returns_value() {
        let svc = CorrectService;
        let result = CacheServiceExt::get(&svc, &"key".to_string()).await.unwrap();
        assert_eq!(*result.unwrap().value(), 42);
    }

    #[tokio::test]
    async fn ext_insert_returns_ok() {
        let svc = CorrectService;
        CacheServiceExt::insert(&svc, &"key".to_string(), CacheEntry::new(42))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ext_invalidate_returns_ok() {
        let svc = CorrectService;
        CacheServiceExt::invalidate(&svc, &"key".to_string()).await.unwrap();
    }

    #[tokio::test]
    async fn ext_clear_returns_ok() {
        let svc = CorrectService;
        CacheServiceExt::clear(&svc).await.unwrap();
    }

    #[tokio::test]
    async fn ext_get_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::get(&svc, &"key".to_string()).await.unwrap_err();
    }

    #[tokio::test]
    async fn ext_insert_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::insert(&svc, &"key".to_string(), CacheEntry::new(42))
            .await
            .unwrap_err();
    }

    #[tokio::test]
    async fn ext_invalidate_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::invalidate(&svc, &"key".to_string()).await.unwrap_err();
    }

    #[tokio::test]
    async fn ext_clear_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::clear(&svc).await.unwrap_err();
    }
}
