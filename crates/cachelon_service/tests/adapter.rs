// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
                Ok(CacheResponse::Insert())
            }
            CacheOperation::Invalidate(req) => {
                let mut data = self.data.lock().expect("lock poisoned");
                data.remove(&req.key);
                Ok(CacheResponse::Invalidate())
            }
            CacheOperation::Clear => {
                let mut data = self.data.lock().expect("lock poisoned");
                data.clear();
                Ok(CacheResponse::Clear())
            }
        }
    }
}

#[tokio::test]
async fn adapter_integrates_with_cache_tier_trait() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);

    // Test CacheTier operations through the adapter
    assert!(adapter.get(&"key".to_string()).await.unwrap().is_none());

    adapter.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

    let result = adapter.get(&"key".to_string()).await;
    assert!(result.is_ok());
    assert_eq!(*result.unwrap().unwrap().value(), 42);

    adapter.invalidate(&"key".to_string()).await.unwrap();
    assert!(adapter.get(&"key".to_string()).await.unwrap().is_none());
}

#[tokio::test]
async fn adapter_operations_return_ok() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);

    // get on missing key
    let result = adapter.get(&"key".to_string()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    // insert
    adapter.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

    // get on existing key
    let result = adapter.get(&"key".to_string()).await.unwrap();
    assert!(result.is_some());

    // invalidate
    adapter.invalidate(&"key".to_string()).await.unwrap();

    // clear
    adapter.clear().await.unwrap();
}

#[tokio::test]
async fn adapter_clear_removes_all_entries() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);

    adapter.insert(&"key1".to_string(), CacheEntry::new(1)).await.unwrap();
    adapter.insert(&"key2".to_string(), CacheEntry::new(2)).await.unwrap();
    adapter.insert(&"key3".to_string(), CacheEntry::new(3)).await.unwrap();

    adapter.clear().await.unwrap();
    assert!(adapter.get(&"key1".to_string()).await.unwrap().is_none());
    assert!(adapter.get(&"key2".to_string()).await.unwrap().is_none());
    assert!(adapter.get(&"key3".to_string()).await.unwrap().is_none());
}

#[test]
fn adapter_len_returns_none() {
    let service = InMemoryCacheService::<String, i32>::new();
    let adapter = ServiceAdapter::new(service);
    assert_eq!(adapter.len(), None);
}

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
fn cache_response_into_enextracts_value() {
    let response = CacheResponse::Get(Some(CacheEntry::new(42)));
    let entry = response.into_entry();
    assert!(entry.is_some());
    assert_eq!(*entry.unwrap().value(), 42);
}

#[test]
fn cache_response_into_enreturns_none_for_non_get() {
    let response: CacheResponse<i32> = CacheResponse::Insert();
    assert!(response.into_entry().is_none());

    let response: CacheResponse<i32> = CacheResponse::Invalidate();
    assert!(response.into_entry().is_none());

    let response: CacheResponse<i32> = CacheResponse::Clear();
    assert!(response.into_entry().is_none());
}
