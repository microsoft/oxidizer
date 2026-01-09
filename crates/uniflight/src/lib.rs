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
//! let group: Merger<&str, String> = Merger::new();
//!
//! // Multiple concurrent calls with the same key will share a single execution
//! let result = group.work("user:123", || async {
//!     // This expensive operation runs only once, even if called concurrently
//!     "expensive_result".to_string()
//! }).await;
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
//! # Thread Safety
//!
//! [`Merger`] is `Send` and `Sync`, and can be shared across threads. The returned futures
//! are `Send` when the closure, future, key, and value types are `Send`.
//!
//! # Performance
//!
//! Benchmarks comparing `uniflight` against `singleflight-async` show the following characteristics:
//!
//! - **Concurrent workloads** (10+ tasks): uniflight is 1.2-1.3x faster, demonstrating better scalability under contention
//! - **Single calls**: singleflight-async has lower per-call overhead (~2x faster for individual operations)
//! - **Multiple keys**: uniflight performs 1.3x faster when handling multiple distinct keys concurrently
//!
//! uniflight's DashMap-based architecture provides excellent scaling properties for high-concurrency scenarios,
//! making it well-suited for production workloads with concurrent access patterns. For low-contention scenarios
//! with predominantly single calls, the performance difference is minimal (sub-microsecond range).

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/favicon.ico")]

use std::{
    fmt::Debug,
    hash::Hash,
    sync::{Arc, Weak},
};

use async_once_cell::OnceCell;
use dashmap::{DashMap, Entry::{Occupied, Vacant}};

/// Represents a class of work and creates a space in which units of work
/// can be executed with duplicate suppression.
pub struct Merger<K, T> {
    mapping: DashMap<K, Weak<OnceCell<T>>>,
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
            mapping: DashMap::new(),
        }
    }
}

impl<K, T> Merger<K, T>
where
    K: Hash + Eq + Clone + Send + Sync,
    T: Send + Sync,
{
    /// Creates a new `Merger` instance.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute and return the value for a given function, making sure that only one
    /// operation is in-flight at a given moment. If a duplicate call comes in,
    /// that caller will wait until the leader completes and return the same value.
    pub fn work<F, Fut>(&self, key: K, func: F) -> impl Future<Output = T> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = T> + Send,
        T: Clone,
    {
        let cell = self.get_or_create_cell(&key);
        let mapping = &self.mapping;
        async move {
            let result = cell.get_or_init(func()).await.clone();
            // Clean up expired weak reference if present
            // Use remove_if to atomically check and remove
            mapping.remove_if(&key, |_, weak| weak.upgrade().is_none());
            result
        }
    }

    /// Gets an existing `OnceCell` for the key, or creates a new one.
    fn get_or_create_cell(&self, key: &K) -> Arc<OnceCell<T>> {
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
        match self.mapping.entry(key.clone()) {
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
