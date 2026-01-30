// Copyright (c) Microsoft Corporation.

//! Integration tests for `ServiceAdapter`.

use cachelon_service::{CacheOperation, CacheResponse, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
use cachelon_tier::{CacheEntry, CacheTier, Error};
use layered::Service;
use std::collections::HashMap;
use std::sync::Mutex;

/// A simple in-memory cache service for testing.
#[derive(Debug)]
struct InMemoryCacheService<K, V> {
    data: Mutex<HashMap<K, CacheEntry<V>>>,
}

impl<K, V> InMemoryCacheService<K, V> {
    fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl<K, V> Service<CacheOperation<K, V>> for InMemoryCacheService<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    type Out = Result<CacheResponse<V>, Error>;

    async fn execute(&self, input: CacheOperation<K, V>) -> Self::Out {
        match input {
            CacheOperation::Get(req) => {
                let data = self.data.lock().expect("lock poisoned");
                Ok(CacheResponse::Get(data.get(&req.key).cloned()))
            }
            CacheOperation::Insert(req) => {
                let mut data = self.data.lock().expect("lock poisoned");
                data.insert(req.key, req.entry);
                Ok(CacheResponse::Insert(()))
            }
            CacheOperation::Invalidate(req) => {
                let mut data = self.data.lock().expect("lock poisoned");
                data.remove(&req.key);
                Ok(CacheResponse::Invalidate(()))
            }
            CacheOperation::Clear => {
                let mut data = self.data.lock().expect("lock poisoned");
                data.clear();
                Ok(CacheResponse::Clear(()))
            }
        }
    }
}

#[tokio::test]
async fn adapter_integrates_with_cache_tier_trait() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);

    // Test CacheTier operations through the adapter
    assert!(adapter.get(&"key".to_string()).await.is_none());

    adapter.insert(&"key".to_string(), CacheEntry::new(42)).await;

    let result = adapter.get(&"key".to_string()).await;
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 42);

    adapter.invalidate(&"key".to_string()).await;
    assert!(adapter.get(&"key".to_string()).await.is_none());
}

#[tokio::test]
async fn adapter_try_operations_return_ok() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);

    // try_get on missing key
    let result = adapter.try_get(&"key".to_string()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    // try_insert
    adapter.try_insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

    // try_get on existing key
    let result = adapter.try_get(&"key".to_string()).await.unwrap();
    assert!(result.is_some());

    // try_invalidate
    adapter.try_invalidate(&"key".to_string()).await.unwrap();

    // try_clear
    adapter.try_clear().await.unwrap();
}

#[tokio::test]
async fn adapter_clear_removes_all_entries() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);

    adapter.insert(&"key1".to_string(), CacheEntry::new(1)).await;
    adapter.insert(&"key2".to_string(), CacheEntry::new(2)).await;
    adapter.insert(&"key3".to_string(), CacheEntry::new(3)).await;

    adapter.clear().await;

    assert!(adapter.get(&"key1".to_string()).await.is_none());
    assert!(adapter.get(&"key2".to_string()).await.is_none());
    assert!(adapter.get(&"key3".to_string()).await.is_none());
}

#[test]
fn adapter_len_returns_none() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);
    assert_eq!(adapter.len(), None);
}

#[test]
fn adapter_is_empty_returns_none() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);
    assert_eq!(adapter.is_empty(), None);
}

#[test]
fn adapter_provides_access_to_inner_service() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter: ServiceAdapter<String, i32, _> = ServiceAdapter::new(service);

    let _inner = adapter.inner();
    let _owned = adapter.into_inner();
}

// Request type tests

#[test]
fn get_request_holds_key() {
    let req = GetRequest::new("test-key".to_string());
    assert_eq!(req.key, "test-key");
}

#[test]
fn insert_request_holds_key_and_entry() {
    let entry = CacheEntry::new(42);
    let req = InsertRequest::new("test-key".to_string(), entry);
    assert_eq!(req.key, "test-key");
    assert_eq!(*req.entry.value(), 42);
}

#[test]
fn invalidate_request_holds_key() {
    let req = InvalidateRequest::new("test-key".to_string());
    assert_eq!(req.key, "test-key");
}

// Response type tests

#[test]
fn cache_response_is_hit_for_some() {
    let response = CacheResponse::Get(Some(CacheEntry::new(42)));
    assert!(response.is_hit());
    assert!(!response.is_miss());
}

#[test]
fn cache_response_is_miss_for_none() {
    let response: CacheResponse<i32> = CacheResponse::Get(None);
    assert!(response.is_miss());
    assert!(!response.is_hit());
}

#[test]
fn cache_response_into_entry_extracts_value() {
    let response = CacheResponse::Get(Some(CacheEntry::new(42)));
    let entry = response.into_entry();
    assert!(entry.is_some());
    assert_eq!(*entry.unwrap().value(), 42);
}

#[test]
fn cache_response_into_entry_returns_none_for_non_get() {
    let response: CacheResponse<i32> = CacheResponse::Insert(());
    assert!(response.into_entry().is_none());

    let response: CacheResponse<i32> = CacheResponse::Invalidate(());
    assert!(response.into_entry().is_none());

    let response: CacheResponse<i32> = CacheResponse::Clear(());
    assert!(response.into_entry().is_none());
}
