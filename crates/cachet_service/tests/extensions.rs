// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Extensions through public APIs.

#![cfg(test)]

mod ext {
    use std::future::ready;

    use cachet_service::{CacheOperation, CacheResponse, CacheServiceExt};
    use cachet_tier::{CacheEntry, Error, InsertOutcome};
    use layered::Service;

    // A correct service that returns expected response types
    #[derive(Debug)]
    struct CorrectService;

    impl Service<CacheOperation<String, i32>> for CorrectService {
        type Out = Result<CacheResponse<i32>, Error>;

        fn execute(&self, input: CacheOperation<String, i32>) -> impl Future<Output = Self::Out> + Send {
            ready(match input {
                CacheOperation::Get(_) => Ok(CacheResponse::Get(Some(CacheEntry::new(42)))),
                CacheOperation::Insert(_) => Ok(CacheResponse::Insert(InsertOutcome::Accepted)),
                CacheOperation::Invalidate(_) => Ok(CacheResponse::Invalidate),
                CacheOperation::Clear => Ok(CacheResponse::Clear),
            })
        }
    }

    // Service that returns wrong response types
    #[derive(Debug)]
    struct WrongResponseService;

    impl Service<CacheOperation<String, i32>> for WrongResponseService {
        type Out = Result<CacheResponse<i32>, Error>;

        fn execute(&self, input: CacheOperation<String, i32>) -> impl Future<Output = Self::Out> + Send {
            ready(match input {
                CacheOperation::Insert(_) => Ok(CacheResponse::Clear),
                CacheOperation::Get(_) | CacheOperation::Invalidate(_) => Ok(CacheResponse::Insert(InsertOutcome::Accepted)),
                CacheOperation::Clear => Ok(CacheResponse::Get(None)),
            })
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
    async fn ext_insert_returns_outcome() {
        let svc = CorrectService;
        let outcome = CacheServiceExt::insert(&svc, "key".to_string(), CacheEntry::new(42)).await.unwrap();

        assert_eq!(outcome, InsertOutcome::Accepted);
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
