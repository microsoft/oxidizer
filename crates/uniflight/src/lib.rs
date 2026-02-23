// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
//! let result = group
//!     .execute("user:123", || async {
//!         // This expensive operation runs only once, even if called concurrently
//!         "expensive_result".to_string()
//!     })
//!     .await
//!     .expect("leader should not panic");
//! # }
//! ```
//!
//! # Flexible Key Types
//!
//! The [`Merger::execute`] method accepts keys using [`Borrow`] semantics, allowing you to pass
//! borrowed forms of the key type. For example, with `Merger<String, T>`, you can pass `&str`
//! directly without allocating:
//!
//! ```
//! # use uniflight::Merger;
//! # async fn example() {
//! let merger: Merger<String, i32> = Merger::new();
//!
//! // Pass &str directly - no need to call .to_string()
//! let result = merger.execute("my-key", || async { 42 }).await;
//! assert_eq!(result, Ok(42));
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
//! use thread_aware::PerNuma;
//! use uniflight::Merger;
//!
//! # async fn example() {
//! // NUMA-aware merger - each NUMA node gets its own deduplication scope
//! let merger: Merger<String, String, PerNuma> = Merger::new_per_numa();
//! # }
//! ```
//!
//! # Cancellation and Panic Handling
//!
//! `Merger` handles task cancellation and panics explicitly:
//!
//! - If the leader task is cancelled or dropped, a follower becomes the new leader
//! - If the leader task panics, followers receive [`LeaderPanicked`] error with the panic message
//! - Followers that join before the leader completes receive the value the leader returns
//!
//! When a panic occurs, followers are notified via the error type rather than silently
//! retrying. The panic message is captured and available via [`LeaderPanicked::message`]:
//!
//! ```
//! # use uniflight::Merger;
//! # async fn example() {
//! let merger: Merger<String, String> = Merger::new();
//! match merger
//!     .execute("key", || async { "result".to_string() })
//!     .await
//! {
//!     Ok(value) => println!("got {value}"),
//!     Err(err) => {
//!         println!("leader panicked: {}", err.message());
//!         // Decide whether to retry
//!     }
//! }
//! # }
//! ```
//!
//! # Memory Management
//!
//! Completed entries are automatically removed from the internal map when the last caller
//! finishes. This ensures no stale entries accumulate over time.
//!
//! # Type Requirements
//!
//! The value type `T` must implement [`Clone`] because followers receive a clone of the
//! leader's result. The key type `K` must implement [`Hash`] and [`Eq`].
//!
//! # Thread Safety
//!
//! [`Merger`] is `Send` and `Sync`, and can be shared across threads. The returned futures
//! are `Send` when the closure, future, key, and value types are `Send`.
//!
//! # Performance
//!
//! Run benchmarks with `cargo bench -p uniflight`. The suite covers:
//!
//! - `single_call`: Baseline latency with no contention
//! - `high_contention_100`: 100 concurrent tasks on the same key
//! - `distributed_10x10`: 10 keys with 10 tasks each
//!
//! Use `--save-baseline` and `--baseline` flags to track regressions over time.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/favicon.ico")]

use std::borrow::Borrow;
use std::fmt::Debug;
use std::hash::Hash;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Weak};

use ahash::RandomState;
use async_once_cell::OnceCell;
use dashmap::DashMap;
use dashmap::Entry::{Occupied, Vacant};
use futures_util::FutureExt; // catch_unwind, map
use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};
use thread_aware::storage::Strategy;
use thread_aware::{Arc as TaArc, PerCore, PerNuma, PerProcess, ThreadAware};

/// Suppresses duplicate async operations identified by a key.
///
/// The `S` type parameter controls the thread-aware scoping strategy:
/// - [`PerProcess`]: Single global scope (default, maximum deduplication)
/// - [`PerNuma`]: Per-NUMA-node scope (NUMA-local memory access)
/// - [`PerCore`]: Per-core scope (no deduplication)
pub struct Merger<K, T, S: Strategy = PerProcess> {
    inner: TaArc<DashMap<K, Weak<PanicAwareCell<T>>, RandomState>, S>,
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
    T: Send + Sync + 'static,
    S: Strategy,
{
    fn default() -> Self {
        Self {
            inner: TaArc::new(|| DashMap::with_hasher(RandomState::new())),
        }
    }
}

impl<K, T, S> Merger<K, T, S>
where
    K: Hash + Eq + Send + Sync + 'static,
    T: Send + Sync + 'static,
    S: Strategy,
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
    /// use thread_aware::{PerCore, PerNuma};
    /// use uniflight::Merger;
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
    T: Send + Sync + 'static,
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
    T: Send + Sync + 'static,
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
    T: Send + Sync + 'static,
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
    T: Send + Sync,
    S: Strategy + Send + Sync,
{
    /// Execute and return the value for a given function, making sure that only one
    /// operation is in-flight at a given moment. If a duplicate call comes in,
    /// that caller will wait until the leader completes and return the same value.
    ///
    /// # Errors
    ///
    /// Returns [`LeaderPanicked`] if the leader task panicked during execution.
    /// Callers can retry by calling `execute` again if desired.
    ///
    /// # Example
    ///
    /// The key can be passed as any borrowed form of `K`. For example, if `K` is `String`,
    /// you can pass `&str` directly:
    ///
    /// ```
    /// # use uniflight::Merger;
    /// # async fn example() {
    /// let merger: Merger<String, i32> = Merger::new();
    /// let result = merger.execute("my-key", || async { 42 }).await;
    /// assert_eq!(result, Ok(42));
    /// # }
    /// ```
    pub fn execute<Q, F, Fut>(&self, key: &Q, func: F) -> impl Future<Output = Result<T, LeaderPanicked>> + Send + use<Q, F, Fut, K, T, S>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = T> + Send,
        T: Clone,
    {
        // Clone the TaArc - the async block owns this clone
        let inner = self.inner.clone();
        let cell = Self::get_or_create_cell(&inner, key);
        let owned_key = key.to_owned();
        async move {
            // Box the future immediately to keep state machine size small.
            // Without boxing, the entire Fut type would be embedded in our state machine.
            // With boxing, we only store a 16-byte pointer.
            let boxed = Box::pin(func());
            let result = cell.get_or_init(boxed).await.clone();
            drop(cell); // Release our Arc before cleanup check
            // Remove entry if no one else is using it (weak can't upgrade)
            inner.remove_if(owned_key.borrow(), |_, weak| weak.upgrade().is_none());
            result
        }
    }

    /// Gets an existing cell for the key, or creates a new one.
    fn get_or_create_cell<Q>(map: &DashMap<K, Weak<PanicAwareCell<T>>, RandomState>, key: &Q) -> Arc<PanicAwareCell<T>>
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
    fn insert_or_get_existing<Q>(map: &DashMap<K, Weak<PanicAwareCell<T>>, RandomState>, key: &Q) -> Arc<PanicAwareCell<T>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
    {
        let cell = Arc::new(PanicAwareCell::new());
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

/// Error returned when the leader task panicked during execution.
///
/// When a leader task panics, followers receive this error instead of
/// silently retrying. Callers can decide whether to retry by calling
/// `execute` again.
///
/// The panic message is captured and available via [`std::fmt::Display`] or [`LeaderPanicked::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderPanicked {
    message: Arc<str>,
}

impl LeaderPanicked {
    /// Returns the panic message from the leader task.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for LeaderPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "leader task panicked: {}", self.message)
    }
}

impl std::error::Error for LeaderPanicked {}

/// Extracts a message from a panic payload.
///
/// Tries to downcast to `&str` or `String`, falling back to a default message.
fn extract_panic_message(payload: &(dyn std::any::Any + Send)) -> Arc<str> {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return Arc::from(*s);
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return Arc::from(s.as_str());
    }
    Arc::from("unknown panic")
}

struct PanicAwareCell<T> {
    inner: OnceCell<Result<T, LeaderPanicked>>,
}

impl<T> PanicAwareCell<T> {
    fn new() -> Self {
        Self { inner: OnceCell::new() }
    }

    #[expect(clippy::future_not_send, reason = "Send bounds enforced by Merger::execute")]
    async fn get_or_init<F>(&self, f: F) -> &Result<T, LeaderPanicked>
    where
        F: Future<Output = T>,
    {
        // Use map combinator instead of async block to avoid extra state machine
        self.inner
            .get_or_init(AssertUnwindSafe(f).catch_unwind().map(|result| {
                result.map_err(|payload| LeaderPanicked {
                    message: extract_panic_message(&*payload),
                })
            }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use thread_aware::affinity::pinned_affinities;

    use super::*;

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
        let map: DashMap<String, Weak<PanicAwareCell<String>>, RandomState> = DashMap::with_hasher(RandomState::new());
        let existing_cell = Arc::new(PanicAwareCell::new());
        map.insert("key".to_string(), Arc::downgrade(&existing_cell));

        let result = Merger::<String, String>::get_or_create_cell(&map, "key");

        assert!(Arc::ptr_eq(&result, &existing_cell));
    }

    #[test]
    fn replaces_expired_entry() {
        let map: DashMap<String, Weak<PanicAwareCell<String>>, RandomState> = DashMap::with_hasher(RandomState::new());
        let expired_weak = Arc::downgrade(&Arc::new(PanicAwareCell::<String>::new()));
        map.insert("key".to_string(), expired_weak);

        let result = Merger::<String, String>::get_or_create_cell(&map, "key");

        let entry = map.get("key").unwrap();
        assert!(Arc::ptr_eq(&result, &entry.value().upgrade().unwrap()));
    }

    /// Simulates a race where another thread inserted between fast-path check and `entry()`.
    #[test]
    fn race_returns_existing() {
        let map: DashMap<String, Weak<PanicAwareCell<String>>, RandomState> = DashMap::with_hasher(RandomState::new());
        let other_cell = Arc::new(PanicAwareCell::new());
        map.insert("key".to_string(), Arc::downgrade(&other_cell));

        let result = Merger::<String, String>::insert_or_get_existing(&map, "key");

        assert!(Arc::ptr_eq(&result, &other_cell));
    }

    #[tokio::test]
    async fn cleanup_after_completion() {
        let group: Merger<String, String> = Merger::new();
        assert!(group.is_empty());

        // Single call should clean up after completion
        let result = group.execute("key1", || async { "Result".to_string() }).await;
        assert_eq!(result, Ok("Result".to_string()));
        assert!(group.is_empty(), "Map should be empty after single call completes");

        // Multiple concurrent calls should clean up after all complete
        let futures: Vec<_> = (0..10)
            .map(|_| {
                group.execute("key2", || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    "Result".to_string()
                })
            })
            .collect();

        // While in flight, map should have an entry
        assert_eq!(group.len(), 1);

        for fut in futures {
            assert_eq!(fut.await, Ok("Result".to_string()));
        }

        assert!(group.is_empty(), "Map should be empty after all concurrent calls complete");

        // Multiple different keys should all be cleaned up
        let fut1 = group.execute("a", || async { "A".to_string() });
        let fut2 = group.execute("b", || async { "B".to_string() });
        let fut3 = group.execute("c", || async { "C".to_string() });

        assert_eq!(group.len(), 3);

        let (r1, r2, r3) = tokio::join!(fut1, fut2, fut3);
        assert_eq!(r1, Ok("A".to_string()));
        assert_eq!(r2, Ok("B".to_string()));
        assert_eq!(r3, Ok("C".to_string()));

        assert!(group.is_empty(), "Map should be empty after all keys complete");
    }

    #[tokio::test]
    async fn catch_unwind_works() {
        // Verify that catch_unwind actually catches panics in async code
        let result = AssertUnwindSafe(async {
            panic!("test panic");
            #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
            42i32
        })
        .catch_unwind()
        .await;

        assert!(result.is_err(), "catch_unwind should catch the panic");
    }

    #[tokio::test]
    async fn panic_aware_cell_catches_panic() {
        let cell = PanicAwareCell::<String>::new();
        let result = cell
            .get_or_init(async {
                panic!("test panic");
                #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
                "never".to_string()
            })
            .await;

        let err = result.as_ref().unwrap_err();
        assert_eq!(err.message(), "test panic");
    }

    #[test]
    fn extract_panic_message_from_string() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("owned string panic"));
        let message = extract_panic_message(&*payload);
        assert_eq!(&*message, "owned string panic");
    }

    #[test]
    fn extract_panic_message_unknown_type() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(42i32);
        let message = extract_panic_message(&*payload);
        assert_eq!(&*message, "unknown panic");
    }
}
