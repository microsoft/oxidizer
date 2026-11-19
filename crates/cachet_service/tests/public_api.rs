// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache service requests, adapters, and extension methods through public APIs.

mod request {
    use cachet_service::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest};
    use cachet_tier::{CacheEntry, InsertOutcome};

    #[test]
    fn get_request_new() {
        let req = GetRequest::new("key".to_string());
        assert_eq!(req.key, "key");
    }

    #[test]
    fn insert_request_new() {
        let entry = CacheEntry::new(42);
        let req = InsertRequest::new("key".to_string(), entry);
        assert_eq!(req.key, "key");
        assert_eq!(*req.entry, 42);
    }

    #[test]
    fn invalidate_request_new() {
        let req = InvalidateRequest::new("key".to_string());
        assert_eq!(req.key, "key");
    }

    #[test]
    fn cache_operation_get() {
        let op: CacheOperation<String, i32> = CacheOperation::Get(GetRequest::new("key".to_string()));
        assert!(matches!(op, CacheOperation::Get(_)));
    }

    #[test]
    fn cache_operation_insert() {
        let entry = CacheEntry::new(42);
        let op = CacheOperation::Insert(InsertRequest::new("key".to_string(), entry));
        assert!(matches!(op, CacheOperation::Insert(_)));
    }

    #[test]
    fn cache_operation_invalidate() {
        let op: CacheOperation<String, i32> = CacheOperation::Invalidate(InvalidateRequest::new("key".to_string()));
        assert!(matches!(op, CacheOperation::Invalidate(_)));
    }

    #[test]
    fn cache_operation_clear() {
        let op: CacheOperation<String, i32> = CacheOperation::Clear;
        assert!(matches!(op, CacheOperation::Clear));
    }

    #[test]
    fn cache_response_is_hit() {
        let entry = CacheEntry::new(42);
        let response: CacheResponse<i32> = CacheResponse::Get(Some(entry));
        assert!(response.is_hit());
        assert!(!response.is_miss());
    }

    #[test]
    fn cache_response_is_miss() {
        let response: CacheResponse<i32> = CacheResponse::Get(None);
        assert!(response.is_miss());
        assert!(!response.is_hit());
    }

    #[test]
    fn cache_response_into_entry_with_value() {
        let entry = CacheEntry::new(42);
        let response = CacheResponse::Get(Some(entry));
        let extracted = response.into_entry();
        assert!(extracted.is_some());
        assert_eq!(*extracted.unwrap(), 42);
    }

    #[test]
    fn cache_response_into_entry_without_value() {
        let response: CacheResponse<i32> = CacheResponse::Get(None);
        let extracted = response.into_entry();
        assert!(extracted.is_none());
    }

    #[test]
    fn cache_response_into_entry_non_get() {
        let response: CacheResponse<i32> = CacheResponse::Insert(InsertOutcome::Accepted);
        let extracted = response.into_entry();
        assert!(extracted.is_none());
    }

    #[test]
    fn cache_response_is_hit_false_for_non_get() {
        assert!(!CacheResponse::<i32>::Insert(InsertOutcome::Accepted).is_hit());
        assert!(!CacheResponse::<i32>::Invalidate.is_hit());
        assert!(!CacheResponse::<i32>::Clear.is_hit());
    }

    #[test]
    fn cache_response_is_miss_false_for_non_get() {
        assert!(!CacheResponse::<i32>::Insert(InsertOutcome::Accepted).is_miss());
        assert!(!CacheResponse::<i32>::Invalidate.is_miss());
        assert!(!CacheResponse::<i32>::Clear.is_miss());
    }
}

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

mod adapter {
    use std::future::ready;

    use cachet_service::{CacheOperation, CacheResponse, ServiceAdapter};
    use cachet_tier::{CacheEntry, CacheTier, Error, InsertOutcome};
    use layered::Service;

    // Mock service for testing
    #[derive(Debug, Clone)]
    struct MockService;

    impl Service<CacheOperation<String, i32>> for MockService {
        type Out = Result<CacheResponse<i32>, Error>;

        fn execute(&self, input: CacheOperation<String, i32>) -> impl Future<Output = Self::Out> + Send {
            ready(match input {
                CacheOperation::Get(req) => {
                    if req.key == "existing" {
                        Ok(CacheResponse::Get(Some(CacheEntry::new(42))))
                    } else {
                        Ok(CacheResponse::Get(None))
                    }
                }
                CacheOperation::Insert(_) => Ok(CacheResponse::Insert(InsertOutcome::Accepted)),
                CacheOperation::Invalidate(_) => Ok(CacheResponse::Invalidate),
                CacheOperation::Clear => Ok(CacheResponse::Clear),
            })
        }
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_get_existing() {
        let adapter = ServiceAdapter::new(MockService);
        let result = adapter.get(&"existing".to_string()).await;
        assert!(result.is_ok());
        assert_eq!(*result.unwrap().unwrap().value(), 42);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_get_missing() {
        let adapter = ServiceAdapter::new(MockService);
        let result = adapter.get(&"missing".to_string()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_insert_preserves_acceptance() {
        let adapter = ServiceAdapter::new(MockService);
        let outcome = adapter.insert("key".to_string(), CacheEntry::new(100)).await.unwrap();

        assert_eq!(outcome, InsertOutcome::Accepted);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_insert_preserves_rejection() {
        #[derive(Debug, Clone)]
        struct RejectingService;

        impl Service<CacheOperation<String, i32>> for RejectingService {
            type Out = Result<CacheResponse<i32>, Error>;

            fn execute(&self, input: CacheOperation<String, i32>) -> impl Future<Output = Self::Out> + Send {
                ready(match input {
                    CacheOperation::Insert(_) => Ok(CacheResponse::Insert(InsertOutcome::Rejected)),
                    CacheOperation::Get(_) => Ok(CacheResponse::Get(None)),
                    CacheOperation::Invalidate(_) => Ok(CacheResponse::Invalidate),
                    CacheOperation::Clear => Ok(CacheResponse::Clear),
                })
            }
        }

        let adapter = ServiceAdapter::new(RejectingService);
        let outcome = adapter.insert("key".to_string(), CacheEntry::new(100)).await.unwrap();

        assert_eq!(outcome, InsertOutcome::Rejected);
        assert!(adapter.get(&"key".to_string()).await.unwrap().is_none());
        adapter.invalidate(&"key".to_string()).await.unwrap();
        adapter.clear().await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_invalidate_returns_ok() {
        let adapter = ServiceAdapter::new(MockService);
        let result = adapter.invalidate(&"key".to_string()).await;
        assert!(result.is_ok(), "invalidate should succeed");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_clear_returns_ok() {
        let adapter = ServiceAdapter::new(MockService);
        let result = adapter.clear().await;
        assert!(result.is_ok(), "clear should succeed");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_len() {
        use cachet_tier::SizeErrorKind;

        let adapter = ServiceAdapter::<String, i32, _>::new(MockService);
        let err = adapter.len().await.unwrap_err();
        assert_eq!(err.kind, SizeErrorKind::Unsupported);
    }

    #[test]
    fn adapter_inner_returns_reference() {
        let adapter = ServiceAdapter::<String, i32, _>::new(MockService);
        let _inner: &MockService = adapter.inner();
    }

    // Service that returns wrong response types to exercise the defensive error branches.
    // The type system cannot prevent a Service implementation from returning the wrong
    // CacheResponse variant for a given CacheOperation, so we guard against it at runtime.
    #[derive(Debug, Clone)]
    struct WrongResponseService;

    impl Service<CacheOperation<String, i32>> for WrongResponseService {
        type Out = Result<CacheResponse<i32>, Error>;

        fn execute(&self, input: CacheOperation<String, i32>) -> impl Future<Output = Self::Out> + Send {
            ready(match input {
                // Returns Clear response for Insert requests (wrong type)
                CacheOperation::Insert(_) => Ok(CacheResponse::Clear),
                // Returns Insert response for Get/Invalidate requests (wrong type)
                CacheOperation::Get(_) | CacheOperation::Invalidate(_) => Ok(CacheResponse::Insert(InsertOutcome::Accepted)),
                // Returns Get response for Clear requests (wrong type)
                CacheOperation::Clear => Ok(CacheResponse::Get(None)),
            })
        }
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_get_wrong_response_returns_error() {
        let adapter = ServiceAdapter::new(WrongResponseService);
        let result = adapter.get(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_insert_wrong_response_returns_error() {
        let adapter = ServiceAdapter::new(WrongResponseService);
        let result = adapter.insert("key".to_string(), CacheEntry::new(42)).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_invalidate_wrong_response_returns_error() {
        let adapter = ServiceAdapter::new(WrongResponseService);
        let result = adapter.invalidate(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn adapter_clear_wrong_response_returns_error() {
        let adapter = ServiceAdapter::new(WrongResponseService);
        let result = adapter.clear().await;
        result.unwrap_err();
    }
}
