// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The core trait for cache storage backends.
//!
//! [`CacheTier`] defines the interface that all cache backends must implement.
//! This trait is designed for composition: implement the storage operations,
//! then use `cachelon` to layer on telemetry, TTL, and multi-tier fallback.

use crate::{CacheEntry, Error};

/// Trait for cache tier implementations.
///
/// Implement this trait to create custom cache backends. The cache system
/// wraps these in `CacheWrapper` to add telemetry and TTL support.
///
/// Only `get` and `insert` are required. All other methods have sensible defaults:
/// - `try_get`/`try_insert`: Wrap the infallible versions in `Ok`
/// - `invalidate`/`try_invalidate`: No-op (not all tiers support invalidation)
/// - `clear`/`try_clear`: No-op (not all tiers support clearing)
/// - `len`/`is_empty`: Return `None` (not all tiers track size)
#[cfg_attr(
    any(test, feature = "dynamic-cache"),
    dynosaur::dynosaur(pub(crate) DynCacheTier = dyn(box) CacheTier, bridge(none))
)]
pub trait CacheTier<K, V>: Send + Sync {
    /// Gets a value, returning an error if the operation fails.
    fn get(&self, key: &K) -> impl Future<Output = Result<Option<CacheEntry<V>>, Error>> + Send
    where
        K: Sync;

    /// Inserts a value, returning an error if the operation fails.
    ///
    /// Default implementation wraps `insert()` in `Ok`.
    fn insert(&self, key: &K, entry: CacheEntry<V>) -> impl Future<Output = Result<(), Error>> + Send
    where
        K: Sync,
        V: Send;

    /// Invalidates a value, returning an error if the operation fails.
    ///
    /// Default implementation wraps `invalidate()` in `Ok`.
    fn invalidate(&self, key: &K) -> impl Future<Output = Result<(), Error>> + Send
    where
        K: Sync;

    /// Clears all entries, returning an error if the operation fails.
    fn clear(&self) -> impl Future<Output = Result<(), Error>> + Send;

    /// Returns the number of entries, if supported.
    ///
    /// Returns `None` for implementations that don't track size.
    fn len(&self) -> Option<u64> {
        None
    }

    /// Returns true if the cache is empty.
    ///
    /// Returns `None` for implementations that don't track size.
    fn is_empty(&self) -> Option<bool> {
        self.len().map(|n| n == 0)
    }
}

// Public API tests moved to tests/tier.rs
