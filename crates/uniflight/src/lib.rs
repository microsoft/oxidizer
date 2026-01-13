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
//! | Single call | 723 ns | 664 ns | ~equal |
//! | 10 concurrent tasks | 50 µs | 56 µs | uniflight 1.1x |
//! | 100 concurrent tasks | 177 µs | 190 µs | uniflight 1.1x |
//! | 10 keys × 10 tasks | 176 µs | 230 µs | uniflight 1.3x |
//! | Sequential reuse | 757 ns | 1.0 µs | uniflight 1.3x |
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
use dashmap::{DashMap, Entry::{Occupied, Vacant}};

/// Represents a class of work and creates a space in which units of work
/// can be executed with duplicate suppression.
pub struct Merger<K, T> {
    mapping: Arc<DashMap<K, Weak<OnceCell<T>>>>,
}

impl<K, T> Debug for Merger<K, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Merger")
            .field("mapping", &format_args!("DashMap<...>"))
            .finish()
    }
}

impl<K, T> Default for Merger<K, T>
where
    K: Hash + Eq,
{
    fn default() -> Self {
        Self {
            mapping: Arc::new(DashMap::new()),
        }
    }
}

impl<K, T> Merger<K, T>
where
    K: Hash + Eq,
{
    /// Creates a new `Merger` instance.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of in-flight operations.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.mapping.len()
    }

    /// Returns `true` if there are no in-flight operations.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.mapping.is_empty()
    }
}

impl<K, T> Merger<K, T>
where
    K: Hash + Eq + Send + Sync,
    T: Clone + Send + Sync,
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
    pub fn work<Q, F, Fut>(&self, key: &Q, func: F) -> impl Future<Output = T> + Send + use<Q, F, Fut, K, T>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = T> + Send,
    {
        let cell = self.get_or_create_cell(key);
        let owned_key = key.to_owned();
        let mapping = Arc::clone(&self.mapping);
        async move {
            let result = cell.get_or_init(func()).await.clone();
            drop(cell); // Release our Arc before cleanup check
            // Remove entry if no one else is using it (weak can't upgrade)
            mapping.remove_if(owned_key.borrow(), |_, weak| weak.upgrade().is_none());
            result
        }
    }

    /// Gets an existing `OnceCell` for the key, or creates a new one.
    fn get_or_create_cell<Q>(&self, key: &Q) -> Arc<OnceCell<T>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
    {
        // Fast path: check if entry exists and is still valid
        if let Some(entry) = self.mapping.get(key)
            && let Some(cell) = entry.value().upgrade()
        {
            return cell;
        }

        // Slow path: need to insert or replace expired entry
        let cell = Arc::new(OnceCell::new());
        let weak = Arc::downgrade(&cell);

        // Use Entry enum to atomically check-and-return or insert
        match self.mapping.entry(key.to_owned()) {
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
            .map(|_| group.work("key2", || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                "Result".to_string()
            }))
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
