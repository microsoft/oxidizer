// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The core trait for cache storage backends.
//!
//! [`CacheTier`] defines the interface that all cache backends must implement.
//! This trait is designed for composition: implement the storage operations,
//! then use `cachet` to layer on telemetry, TTL, and multi-tier fallback.

use std::future::Future;

use crate::{CacheEntry, Error, SizeError};

/// Whether a cache tier accepted an insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The tier accepted the insertion.
    Accepted,
    /// The tier rejected the insertion without an error.
    Rejected,
}

/// Trait for cache tier implementations.
///
/// Implement this trait to create custom cache backends. The cache system
/// wraps these in `CacheWrapper` to add telemetry and TTL support.
///
/// # Consistency
///
/// [`Accepted`](InsertOutcome::Accepted) means that this tier, or at least one
/// tier in a composite cache, accepted the write. It does not guarantee that the
/// next read returns that entry. The entry may be evicted or invalidated, and a
/// higher-priority tier in a composite cache may still contain an older value.
/// The outcome is intentionally aggregate and does not describe which child of
/// a composite accepted the write. Individual implementations may document
/// stronger consistency guarantees or expose topology-specific diagnostics
/// separately.
///
/// `len` and `is_empty` have default implementations:
/// - `len`: Returns `Err(SizeError::unsupported())` (not all tiers track size)
/// - `is_empty`: Delegates to [`len`](Self::len), returning `Ok(true)` when
///   the reported length is `0` and otherwise propagating any `SizeError`
#[dynosaur::dynosaur(pub(crate) DynCacheTier = dyn(box) CacheTier, bridge(none))]
#[allow(
    clippy::allow_attributes,
    unreachable_pub,
    reason = "re-exported at the crate root via `pub use tier::CacheTier`; the dynosaur attribute macro misfires unreachable_pub on the definition and item-level #[expect] is unfulfilled across the expansion"
)]
pub trait CacheTier<K, V>: Send + Sync {
    /// Gets a value, returning an error if the operation fails.
    fn get(&self, key: &K) -> impl Future<Output = Result<Option<CacheEntry<V>>, Error>> + Send;

    /// Inserts or replaces a value and reports whether the tier accepted it.
    fn insert(&self, key: K, entry: CacheEntry<V>) -> impl Future<Output = Result<InsertOutcome, Error>> + Send;

    /// Invalidates a value, returning an error if the operation fails.
    fn invalidate(&self, key: &K) -> impl Future<Output = Result<(), Error>> + Send;

    /// Clears all entries, returning an error if the operation fails.
    fn clear(&self) -> impl Future<Output = Result<(), Error>> + Send;

    /// Returns an **approximate** count of entries, if the implementation supports it.
    ///
    /// Returns `Err(SizeError::unsupported())` for implementations that do not track size.
    ///
    /// # Approximation
    ///
    /// The returned count may include entries that have logically expired but have
    /// not yet been evicted. Many implementations perform eviction lazily or on a
    /// background schedule, so `len()` can temporarily over count after TTL expiry
    /// or after `invalidate` / `clear` calls that have not yet been fully applied.
    ///
    /// Do not use this value for exact bookkeeping or correctness decisions. It is
    /// suitable for approximate capacity monitoring, metrics, and health checks.
    ///
    /// # Errors
    ///
    /// Returns `Err(SizeError::unsupported())` if the tier does not support size
    /// reporting.
    /// Returns an error with [`Failed`](crate::SizeErrorKind::Failed) kind if the
    /// underlying storage operation fails.
    fn len(&self) -> impl Future<Output = Result<u64, SizeError>> + Send {
        async { Err(SizeError::unsupported()) }
    }

    /// Returns `Ok(true)` if the cache appears to contain no entries.
    ///
    /// Default implementation delegates to [`len`](Self::len).
    ///
    /// # Errors
    ///
    /// Returns `Err(SizeError::unsupported())` if the tier does not support size
    /// reporting.
    /// Returns an error with [`Failed`](crate::SizeErrorKind::Failed) kind if the
    /// underlying storage operation fails.
    fn is_empty(&self) -> impl Future<Output = Result<bool, SizeError>> + Send {
        async { self.len().await.map(|n| n == 0) }
    }
}
