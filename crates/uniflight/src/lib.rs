// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Based on singleflight-async by ihciah
// Original: https://github.com/ihciah/singleflight-async
// Licensed under MIT/Apache-2.0

//! Coalesces duplicate async tasks into a single execution.
//!
//! This crate provides [`Merger`], a mechanism for deduplicating concurrent async operations.
//! When multiple tasks request the same work (identified by a key), only the first task (the
//! "leader") performs the actual work while subsequent tasks (the "followers") wait and receive
//! a clone of the result.
//!
//! # When to Use
//!
//! Use `Merger` when you have expensive or rate-limited operations that may be requested
//! concurrently with the same parameters:
//!
//! - **Cache population**: Prevent thundering herd when a cache entry expires
//! - **API calls**: Deduplicate concurrent requests to the same endpoint
//! - **Database queries**: Coalesce identical queries issued simultaneously
//! - **File I/O**: Avoid reading the same file multiple times concurrently
//!
//! # Example
//!
//! ```
//! use uniflight::Merger;
//!
//! # async fn example() {
//! let group: Merger<String, String> = Merger::new();
//!
//! // Multiple concurrent calls with the same key will share a single execution.
//! // Note: you can pass &str directly when the key type is String.
//! let result = group.work("user:123", || async {
//!     // This expensive operation runs only once, even if called concurrently
//!     "expensive_result".to_string()
//! }).await;
//! # }
//! ```
//!
//! # Flexible Key Types
//!
//! The [`Merger::work`] method accepts keys using [`Borrow`] semantics, allowing you to pass
//! borrowed forms of the key type. For example, with `Merger<String, T>`, you can pass `&str`
//! directly without allocating:
//!
//! ```
//! # use uniflight::Merger;
//! # async fn example() {
//! let merger: Merger<String, i32> = Merger::new();
//!
//! // Pass &str directly - no need to call .to_string()
//! merger.work("my-key", || async { 42 }).await;
//! # }
//! ```
//!
//! # Thread-Aware Scoping
//!
//! `Merger` supports thread-aware scoping via a [`Strategy`]
//! type parameter. This controls how the internal state is partitioned across threads/NUMA nodes:
//!
//! - [`PerProcess`] (default): Single global state, maximum deduplication
//! - [`PerNuma`]: Separate state per NUMA node, NUMA-local memory access
//! - [`PerCore`]: Separate state per core, no deduplication (useful for already-partitioned work)
//!
//! ```
//! use uniflight::{Merger, PerNuma};
//!
//! # async fn example() {
//! // NUMA-aware merger - each NUMA node gets its own deduplication scope
//! let merger: Merger<String, String, PerNuma> = Merger::new_per_numa();
//! # }
//! ```
//!
//! # Cancellation and Panic Safety
//!
//! `Merger` handles task cancellation and panics gracefully:
//!
//! - If the leader task is cancelled or dropped, a follower becomes the new leader
//! - If the leader task panics, a follower becomes the new leader and executes its work
//! - Followers that join before the leader completes receive the cached result
//!
//! # Memory Management
//!
//! Completed entries are automatically removed from the internal map when the last caller
//! finishes. This ensures no stale entries accumulate over time.
//!
//! # Thread Safety
//!
//! [`Merger`] is `Send` and `Sync`, and can be shared across threads. The returned futures
//! are `Send` when the closure, future, key, and value types are `Send`.
//!
//! # Performance
//!
//! Benchmarks comparing `uniflight` against `singleflight-async`:
//!
//! | Benchmark | uniflight | singleflight-async | Winner |
//! |-----------|-----------|-------------------|--------|
//! | Single call | 777 ns | 691 ns | ~equal |
//! | 10 concurrent tasks | 58 µs | 57 µs | ~equal |
//! | 100 concurrent tasks | 218 µs | 219 µs | ~equal |
//! | 10 keys × 10 tasks | 186 µs | 270 µs | uniflight 1.4x |
//! | Sequential reuse | 799 ns | 759 ns | ~equal |
//!
//! uniflight's `DashMap`-based architecture scales well under contention, making it
//! well-suited for high-concurrency workloads. For single-call scenarios, both libraries
//! perform similarly (sub-microsecond).

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/favicon.ico")]

use std::{
    borrow::Borrow,
    fmt::Debug,
    hash::Hash,
    sync::{Arc, Weak},
};

use async_once_cell::OnceCell;
use dashmap::{
    DashMap,
    Entry::{Occupied, Vacant},
};
use thread_aware::{
    Arc as TaArc, ThreadAware,
    affinity::{MemoryAffinity, PinnedAffinity},
    storage::Strategy,
};

// Re-export strategies for convenience
pub use thread_aware::{PerCore, PerNuma, PerProcess};

/// Represents a class of work and creates a space in which units of work
/// can be executed with duplicate suppression.
///
/// The `S` type parameter controls the thread-aware scoping strategy:
/// - [`PerProcess`]: Single global scope (default, maximum deduplication)
/// - [`PerNuma`]: Per-NUMA-node scope (NUMA-local memory access)
/// - [`PerCore`]: Per-core scope (no deduplication)
pub struct Merger<K, T, S: Strategy = PerProcess> {
    inner: TaArc<DashMap<K, Weak<OnceCell<T>>>, S>,
}

impl<K, T, S: Strategy> Debug for Merger<K, T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Merger").field("inner", &format_args!("DashMap<...>")).finish()
    }
}

impl<K, T, S: Strategy> Clone for Merger<K, T, S> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<K, T, S> Default for Merger<K, T, S>
where
    K: Hash + Eq + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
    S: Strategy + Send + Sync,
{
    fn default() -> Self {
        Self {
            inner: TaArc::new(DashMap::new),
        }
    }
}

impl<K, T, S> Merger<K, T, S>
where
    K: Hash + Eq + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
    S: Strategy + Send + Sync,
{
    /// Creates a new `Merger` instance.
    ///
    /// The scoping strategy is determined by the type parameter `S`:
    /// - [`PerProcess`] (default): Process-wide scope, maximum deduplication
    /// - [`PerNuma`]: Per-NUMA-node scope, NUMA-local memory access
    /// - [`PerCore`]: Per-core scope, no cross-core deduplication
    ///
    /// # Examples
    ///
    /// ```
    /// use uniflight::{Merger, PerNuma, PerCore};
    ///
    /// // Default (PerProcess) - type can be inferred
    /// let global: Merger<String, String> = Merger::new();
    ///
    /// // NUMA-local scope
    /// let numa: Merger<String, String, PerNuma> = Merger::new();
    ///
    /// // Per-core scope
    /// let core: Merger<String, String, PerCore> = Merger::new();
    /// ```
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<K, T> Merger<K, T, PerProcess>
where
    K: Hash + Eq + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
{
    /// Creates a new `Merger` with process-wide scoping (default).
    ///
    /// All threads share a single deduplication scope, providing maximum
    /// work deduplication across the entire process.
    ///
    /// # Example
    ///
    /// ```
    /// use uniflight::Merger;
    ///
    /// let merger = Merger::<String, String, _>::new_per_process();
    /// ```
    #[inline]
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Equivalent mutant: delegates to Default
    pub fn new_per_process() -> Self {
        Self::default()
    }
}

impl<K, T> Merger<K, T, PerNuma>
where
    K: Hash + Eq + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
{
    /// Creates a new `Merger` with per-NUMA-node scoping.
    ///
    /// Each NUMA node gets its own deduplication scope, ensuring memory
    /// locality for cached results while still deduplicating within each node.
    ///
    /// # Example
    ///
    /// ```
    /// use uniflight::Merger;
    ///
    /// let merger = Merger::<String, String, _>::new_per_numa();
    /// ```
    #[inline]
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Equivalent mutant: delegates to Default
    pub fn new_per_numa() -> Self {
        Self::default()
    }
}

impl<K, T> Merger<K, T, PerCore>
where
    K: Hash + Eq + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
{
    /// Creates a new `Merger` with per-core scoping.
    ///
    /// Each core gets its own deduplication scope. This is useful when work
    /// is already partitioned by core and cross-core deduplication is not needed.
    ///
    /// # Example
    ///
    /// ```
    /// use uniflight::Merger;
    ///
    /// let merger = Merger::<String, String, _>::new_per_core();
    /// ```
    #[inline]
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Equivalent mutant: delegates to Default
    pub fn new_per_core() -> Self {
        Self::default()
    }
}

impl<K, T, S: Strategy> Merger<K, T, S>
where
    K: Hash + Eq,
{
    /// Returns the number of in-flight operations.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if there are no in-flight operations.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<K, T, S> ThreadAware for Merger<K, T, S>
where
    S: Strategy,
{
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        Self {
            inner: self.inner.relocated(source, destination),
        }
    }
}

impl<K, T, S> Merger<K, T, S>
where
    K: Hash + Eq + Send + Sync,
    T: Clone + Send + Sync,
    S: Strategy + Send + Sync,
{
    /// Execute and return the value for a given function, making sure that only one
    /// operation is in-flight at a given moment. If a duplicate call comes in,
    /// that caller will wait until the leader completes and return the same value.
    ///
    /// The key can be passed as any borrowed form of `K`. For example, if `K` is `String`,
    /// you can pass `&str` directly:
    ///
    /// ```
    /// # use uniflight::Merger;
    /// # async fn example() {
    /// let merger: Merger<String, i32> = Merger::new();
    /// let result = merger.work("my-key", || async { 42 }).await;
    /// # }
    /// ```
    pub fn work<Q, F, Fut>(&self, key: &Q, func: F) -> impl Future<Output = T> + Send + use<Q, F, Fut, K, T, S>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = T> + Send,
    {
        // Clone the TaArc - the async block owns this clone
        let inner = self.inner.clone();
        let cell = Self::get_or_create_cell(&inner, key);
        let owned_key = key.to_owned();
        async move {
            let result = cell.get_or_init(func()).await.clone();
            drop(cell); // Release our Arc before cleanup check
            // Remove entry if no one else is using it (weak can't upgrade)
            inner.remove_if(owned_key.borrow(), |_, weak| weak.upgrade().is_none());
            result
        }
    }

    /// Gets an existing `OnceCell` for the key, or creates a new one.
    fn get_or_create_cell<Q>(map: &DashMap<K, Weak<OnceCell<T>>>, key: &Q) -> Arc<OnceCell<T>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
    {
        // Fast path: check if entry exists and is still valid
        if let Some(entry) = map.get(key)
            && let Some(cell) = entry.value().upgrade()
        {
            return cell;
        }

        // Slow path: need to insert or replace expired entry
        Self::insert_or_get_existing(map, key)
    }

    /// Inserts a new cell or returns an existing live cell (handling races).
    ///
    /// This is the slow path of `get_or_create_cell`, separated for testability.
    /// It handles the case where another thread may have inserted a cell between
    /// our fast-path check and this insertion attempt.
    fn insert_or_get_existing<Q>(map: &DashMap<K, Weak<OnceCell<T>>>, key: &Q) -> Arc<OnceCell<T>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
    {
        let cell = Arc::new(OnceCell::new());
        let weak = Arc::downgrade(&cell);

        // Use Entry enum to atomically check-and-return or insert
        match map.entry(key.to_owned()) {
            Occupied(mut entry) => {
                // Entry exists - check if still alive
                if let Some(existing) = entry.get().upgrade() {
                    // Another thread's cell is still alive - use it
                    return existing;
                }
                // Expired - replace with ours
                entry.insert(weak);
            }
            Vacant(entry) => {
                entry.insert(weak);
            }
        }

        // We inserted our cell, return it
        cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use thread_aware::affinity::pinned_affinities;

    #[test]
    fn relocated_delegates_to_inner() {
        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        let merger: Merger<String, String> = Merger::new();
        let relocated = merger.relocated(source, destination);

        // Verify the relocated merger still works
        assert!(relocated.is_empty());
    }

    #[test]
    fn fast_path_returns_existing() {
        let map: DashMap<String, Weak<OnceCell<String>>> = DashMap::new();
        let existing_cell = Arc::new(OnceCell::new());
        map.insert("key".to_string(), Arc::downgrade(&existing_cell));

        let result = Merger::<String, String>::get_or_create_cell(&map, "key");

        assert!(Arc::ptr_eq(&result, &existing_cell));
    }

    #[test]
    fn replaces_expired_entry() {
        let map: DashMap<String, Weak<OnceCell<String>>> = DashMap::new();
        let expired_weak = Arc::downgrade(&Arc::new(OnceCell::<String>::new()));
        map.insert("key".to_string(), expired_weak);

        let result = Merger::<String, String>::get_or_create_cell(&map, "key");

        let entry = map.get("key").unwrap();
        assert!(Arc::ptr_eq(&result, &entry.value().upgrade().unwrap()));
    }

    /// Simulates a race where another thread inserted between fast-path check and `entry()`.
    #[test]
    fn race_returns_existing() {
        let map: DashMap<String, Weak<OnceCell<String>>> = DashMap::new();
        let other_cell = Arc::new(OnceCell::new());
        map.insert("key".to_string(), Arc::downgrade(&other_cell));

        let result = Merger::<String, String>::insert_or_get_existing(&map, "key");

        assert!(Arc::ptr_eq(&result, &other_cell));
    }

    #[tokio::test]
    async fn cleanup_after_completion() {
        let group: Merger<String, String> = Merger::new();
        assert!(group.is_empty());

        // Single call should clean up after completion
        let result = group.work("key1", || async { "Result".to_string() }).await;
        assert_eq!(result, "Result");
        assert!(group.is_empty(), "Map should be empty after single call completes");

        // Multiple concurrent calls should clean up after all complete
        let futures: Vec<_> = (0..10)
            .map(|_| {
                group.work("key2", || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    "Result".to_string()
                })
            })
            .collect();

        // While in flight, map should have an entry
        assert_eq!(group.len(), 1);

        for fut in futures {
            assert_eq!(fut.await, "Result");
        }

        assert!(group.is_empty(), "Map should be empty after all concurrent calls complete");

        // Multiple different keys should all be cleaned up
        let fut1 = group.work("a", || async { "A".to_string() });
        let fut2 = group.work("b", || async { "B".to_string() });
        let fut3 = group.work("c", || async { "C".to_string() });

        assert_eq!(group.len(), 3);

        let (r1, r2, r3) = tokio::join!(fut1, fut2, fut3);
        assert_eq!(r1, "A");
        assert_eq!(r2, "B");
        assert_eq!(r3, "C");

        assert!(group.is_empty(), "Map should be empty after all keys complete");
    }
}
