// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use cachet_tier::{CacheEntry, Error, InsertOutcome};
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
    fn insert(&self, key: K, entry: CacheEntry<V>) -> impl Future<Output = Result<(), Error>> + Send;
    /// Inserts a value and reports whether it was accepted.
    ///
    /// The provided implementation reports acceptance after a successful
    /// [`insert`](Self::insert). Implementations that can reject writes without
    /// an error must override this method.
    fn insert_with_outcome(&self, key: K, entry: CacheEntry<V>) -> impl Future<Output = Result<InsertOutcome, Error>> + Send
    where
        Self: Sync,
        K: Send,
        V: Send,
    {
        async {
            self.insert(key, entry).await?;
            Ok(InsertOutcome::Accepted)
        }
    }
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

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.insert_with_outcome(key, entry).await.map(drop)
    }

    async fn insert_with_outcome(&self, key: K, entry: CacheEntry<V>) -> Result<InsertOutcome, Error> {
        let req = InsertRequest { key: key.clone(), entry };
        match self.execute(CacheOperation::Insert(req)).await? {
            CacheResponse::Insert(outcome) => Ok(outcome),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let req = InvalidateRequest { key: key.clone() };
        match self.execute(CacheOperation::Invalidate(req)).await? {
            CacheResponse::Invalidate => Ok(()),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }

    async fn clear(&self) -> Result<(), Error> {
        match self.execute(CacheOperation::Clear).await? {
            CacheResponse::Clear => Ok(()),
            _ => Err(Error::from_message("unexpected response type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Debug, Default)]
    struct LegacyExtension {
        inserted: AtomicBool,
    }

    impl CacheServiceExt<String, i32> for LegacyExtension {
        async fn get(&self, _key: &String) -> Result<Option<CacheEntry<i32>>, Error> {
            Ok(None)
        }

        async fn insert(&self, _key: String, _entry: CacheEntry<i32>) -> Result<(), Error> {
            self.inserted.store(true, Ordering::Relaxed);
            Ok(())
        }

        async fn invalidate(&self, _key: &String) -> Result<(), Error> {
            Ok(())
        }

        async fn clear(&self) -> Result<(), Error> {
            Ok(())
        }
    }

    // A correct service that returns expected response types
    #[derive(Debug)]
    struct CorrectService;

    impl Service<CacheOperation<String, i32>> for CorrectService {
        type Out = Result<CacheResponse<i32>, Error>;

        async fn execute(&self, input: CacheOperation<String, i32>) -> Self::Out {
            match input {
                CacheOperation::Get(_) => Ok(CacheResponse::Get(Some(CacheEntry::new(42)))),
                CacheOperation::Insert(_) => Ok(CacheResponse::Insert(InsertOutcome::Accepted)),
                CacheOperation::Invalidate(_) => Ok(CacheResponse::Invalidate),
                CacheOperation::Clear => Ok(CacheResponse::Clear),
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
                CacheOperation::Insert(_) => Ok(CacheResponse::Clear),
                CacheOperation::Get(_) | CacheOperation::Invalidate(_) => Ok(CacheResponse::Insert(InsertOutcome::Accepted)),
                CacheOperation::Clear => Ok(CacheResponse::Get(None)),
            }
        }
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_get_returns_value() {
        let svc = CorrectService;
        let result = CacheServiceExt::get(&svc, &"key".to_string()).await.unwrap();
        assert_eq!(*result.unwrap().value(), 42);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_insert_returns_ok() {
        let svc = CorrectService;
        CacheServiceExt::insert(&svc, "key".to_string(), CacheEntry::new(42)).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_insert_with_outcome_returns_outcome() {
        let svc = CorrectService;
        let outcome = CacheServiceExt::insert_with_outcome(&svc, "key".to_string(), CacheEntry::new(42))
            .await
            .unwrap();
        assert_eq!(outcome, InsertOutcome::Accepted);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_default_insert_with_outcome_delegates_to_insert() {
        let extension = LegacyExtension::default();

        let outcome = extension.insert_with_outcome("key".to_string(), CacheEntry::new(42)).await.unwrap();

        assert_eq!(outcome, InsertOutcome::Accepted);
        assert!(extension.inserted.load(Ordering::Relaxed));
        assert!(extension.get(&"key".to_string()).await.unwrap().is_none());
        extension.invalidate(&"key".to_string()).await.unwrap();
        extension.clear().await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_invalidate_returns_ok() {
        let svc = CorrectService;
        CacheServiceExt::invalidate(&svc, &"key".to_string()).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_clear_returns_ok() {
        let svc = CorrectService;
        CacheServiceExt::clear(&svc).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_get_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::get(&svc, &"key".to_string()).await.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_insert_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::insert(&svc, "key".to_string(), CacheEntry::new(42))
            .await
            .unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_invalidate_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::invalidate(&svc, &"key".to_string()).await.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ext_clear_wrong_response_returns_error() {
        let svc = WrongResponseService;
        CacheServiceExt::clear(&svc).await.unwrap_err();
    }
}
