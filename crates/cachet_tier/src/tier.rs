// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The core trait for cache storage backends.
//!
//! [`CacheTier`] defines the interface that all cache backends must implement.
//! This trait is designed for composition: implement the storage operations,
//! then use `cachet` to layer on telemetry, TTL, and multi-tier fallback.

use std::future::Future;

use crate::{CacheEntry, Error};

/// Trait for cache tier implementations.
///
/// Implement this trait to create custom cache backends. The cache system
/// wraps these in `CacheWrapper` to add telemetry and TTL support.
///
/// All four core methods are required: `get`, `insert`, `invalidate`, and `clear`.
/// Only `len` and `is_empty` have default implementations:
/// - `len`: Returns `None` (not all tiers track size)
/// - `is_empty`: Delegates to `len`
#[dynosaur::dynosaur(pub(crate) DynCacheTier = dyn(box) CacheTier, bridge(none))]
pub trait CacheTier<K, V>: Send + Sync {
    /// Gets a value, returning an error if the operation fails.
    fn get(&self, key: &K) -> impl Future<Output = Result<Option<CacheEntry<V>>, Error>> + Send;

    /// Inserts a value, returning an error if the operation fails.
    fn insert(&self, key: &K, entry: CacheEntry<V>) -> impl Future<Output = Result<(), Error>> + Send;

    /// Invalidates a value, returning an error if the operation fails.
    fn invalidate(&self, key: &K) -> impl Future<Output = Result<(), Error>> + Send;

    /// Clears all entries, returning an error if the operation fails.
    fn clear(&self) -> impl Future<Output = Result<(), Error>> + Send;

    /// Returns an **approximate** count of entries, if the implementation supports it.
    ///
    /// Returns `None` for implementations that do not track size.
    ///
    /// # Approximation
    ///
    /// The returned count may include entries that have logically expired but have
    /// not yet been evicted. Many implementations perform eviction lazily or on a
    /// background schedule, so `len()` can temporarily overcount after TTL expiry
    /// or after `invalidate` / `clear` calls that have not yet been fully applied.
    ///
    /// Do not use this value for exact bookkeeping or correctness decisions. It is
    /// suitable for approximate capacity monitoring, metrics, and health checks.
    fn len(&self) -> Option<u64> {
        None
    }

    /// Returns `true` if the cache **appears** to contain no entries.
    ///
    /// Returns `None` for implementations that do not track size.
    ///
    /// Subject to the same approximation caveat as [`len`](Self::len): a return
    /// value of `false` does not guarantee that a subsequent `get` will find anything,
    /// and a return value of `true` does not guarantee the cache is actually empty if
    /// entries have expired but not yet been evicted.
    fn is_empty(&self) -> Option<bool> {
        self.len().map(|len| len == 0)
    }
}
