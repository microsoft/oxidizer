// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Requests through public APIs.

#![cfg(test)]

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
