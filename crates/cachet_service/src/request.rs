// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request and response types for cache operations exposed through the Service trait.

use cachet_tier::{CacheEntry, InsertOutcome};

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
    /// Response from an insert operation, including whether it was accepted
    Insert(InsertOutcome),
    /// Response from an invalidate operation
    Invalidate,
    /// Response from a clear operation
    Clear,
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
