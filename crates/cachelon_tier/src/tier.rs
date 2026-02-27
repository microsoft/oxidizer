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
/// All four core methods are required: `get`, `insert`, `invalidate`, and `clear`.
/// Only `len` and `is_empty` have default implementations:
/// - `len`: Returns `None` (not all tiers track size)
/// - `is_empty`: Delegates to `len`
#[cfg_attr(
    any(test, feature = "dynamic-cache"),
    dynosaur::dynosaur(pub(crate) DynCacheTier = dyn(box) CacheTier, bridge(none))
)]
pub trait CacheTier<K, V>: Send + Sync {
    /// Gets a value, returning an error if the operation fails.
    fn get(&self, key: &K) -> impl Future<Output = Result<Option<CacheEntry<V>>, Error>> + Send;

    /// Inserts a value, returning an error if the operation fails.
    fn insert(&self, key: &K, entry: CacheEntry<V>) -> impl Future<Output = Result<(), Error>> + Send;

    /// Invalidates a value, returning an error if the operation fails.
    fn invalidate(&self, key: &K) -> impl Future<Output = Result<(), Error>> + Send;

    /// Clears all entries, returning an error if the operation fails.
    fn clear(&self) -> impl Future<Output = Result<(), Error>> + Send;

    /// Returns the number of entries, if supported.
    ///
    /// Returns `None` for implementations that don't track size.
    fn len(&self) -> Option<u64> {
        None
    }

    /// Returns `true` if the cache contains no entries.
    ///
    /// Returns `None` for implementations that don't track size.
    fn is_empty(&self) -> Option<bool> {
        self.len().map(|len| len == 0)
    }
}
