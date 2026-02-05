// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request and response types for cache operations exposed through the Service trait.

use cachelon_tier::CacheEntry;

/// A cache operation request.
///
/// This enum represents all possible cache operations that can be performed
/// through the `Service` trait. It enables composing cache operations with
/// middleware like retry, timeout, and circuit breakers.
#[derive(Debug, Clone)]
pub enum CacheOperation<K, V> {
    /// Get a value from the cache
    Get(GetRequest<K>),
    /// Insert a value into the cache
    Insert(InsertRequest<K, V>),
    /// Invalidate (remove) a value from the cache
    Invalidate(InvalidateRequest<K>),
    /// Clear all entries from the cache
    Clear,
}

/// Request to get a value from the cache.
#[derive(Debug, Clone)]
pub struct GetRequest<K> {
    /// The key to retrieve
    pub key: K,
}

impl<K> GetRequest<K> {
    /// Creates a new get request for the given key.
    #[must_use]
    pub fn new(key: K) -> Self {
        Self { key }
    }
}

/// Request to insert a value into the cache.
#[derive(Debug, Clone)]
pub struct InsertRequest<K, V> {
    /// The key to insert
    pub key: K,
    /// The entry to store (includes value and metadata)
    pub entry: CacheEntry<V>,
}

impl<K, V> InsertRequest<K, V> {
    /// Creates a new insert request for the given key and entry.
    #[must_use]
    pub fn new(key: K, entry: CacheEntry<V>) -> Self {
        Self { key, entry }
    }
}

/// Request to invalidate (remove) a value from the cache.
#[derive(Debug, Clone)]
pub struct InvalidateRequest<K> {
    /// The key to invalidate
    pub key: K,
}

impl<K> InvalidateRequest<K> {
    /// Creates a new invalidate request for the given key.
    #[must_use]
    pub fn new(key: K) -> Self {
        Self { key }
    }
}

/// Response from a cache operation.
///
/// Each variant corresponds to the result of a cache operation.
#[derive(Debug, Clone)]
pub enum CacheResponse<V> {
    /// Response from a get operation
    Get(Option<CacheEntry<V>>),
    /// Response from an insert operation
    Insert(),
    /// Response from an invalidate operation
    Invalidate(),
    /// Response from a clear operation
    Clear(),
}

impl<V> CacheResponse<V> {
    /// Returns `true` if this response represents a cache hit (Get with Some value).
    #[must_use]
    pub fn is_hit(&self) -> bool {
        matches!(self, Self::Get(Some(_)))
    }

    /// Returns `true` if this response represents a cache miss (Get with None).
    #[must_use]
    pub fn is_miss(&self) -> bool {
        matches!(self, Self::Get(None))
    }

    /// Extracts the entry from a Get response, if present.
    #[must_use]
    pub fn into_entry(self) -> Option<CacheEntry<V>> {
        match self {
            Self::Get(entry) => entry,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let response: CacheResponse<i32> = CacheResponse::Insert();
        let extracted = response.into_entry();
        assert!(extracted.is_none());
    }
}
