// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Based on singleflight-async by ihciah
// Original: https://github.com/ihciah/singleflight-async
// Licensed under MIT/Apache-2.0

//! Coalesces duplicate async tasks into a single execution.
//!
//! This crate provides [`UniFlight`], a mechanism for deduplicating concurrent async operations.
//! When multiple tasks request the same work (identified by a key), only the first task (the
//! "leader") performs the actual work while subsequent tasks (the "followers") wait and receive
//! a clone of the result.
//!
//! # When to Use
//!
//! Use `UniFlight` when you have expensive or rate-limited operations that may be requested
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
//! use uniflight::UniFlight;
//!
//! # async fn example() {
//! let group: UniFlight<&str, String> = UniFlight::new();
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
//! `UniFlight` handles task cancellation and panics gracefully:
//!
//! - If the leader task is cancelled or dropped, a follower becomes the new leader
//! - If the leader task panics, a follower becomes the new leader and executes its work
//! - Followers that join before the leader completes receive the cached result
//!
//! # Thread Safety
//!
//! [`UniFlight`] is `Send` and `Sync`, and can be shared across threads. The returned futures
//! do not require `Send` bounds on the closure or its output.
//!
//! # Multiple Leaders for Redundancy
//!
//! By default, `UniFlight` uses a single leader per key. For redundancy scenarios where you want
//! multiple concurrent attempts at the same operation (using whichever completes first), use
//! [`UniFlight::with_max_leaders`]:
//!
//! ```
//! use uniflight::UniFlight;
//!
//! # async fn example() {
//! // Allow up to 3 concurrent leaders for redundancy
//! let group: UniFlight<&str, String> = UniFlight::with_max_leaders(3);
//!
//! // First 3 concurrent calls become leaders and execute in parallel.
//! // The first leader to complete stores the result.
//! // All callers (leaders and followers) receive that result.
//! let result = group.work("key", || async {
//!     "result".to_string()
//! }).await;
//! # }
//! ```
//!
//! This is useful when:
//! - You want fault tolerance through redundant execution
//! - Network latency varies and you want the fastest response
//! - You're implementing speculative execution patterns

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/favicon.ico")]

use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

use parking_lot::Mutex as SyncMutex;
use xutex::AsyncMutex;

type SharedMapping<K, T> = Arc<SyncMutex<HashMap<K, BroadcastOnce<T>>>>;

/// Represents a class of work and creates a space in which units of work
/// can be executed with duplicate suppression.
#[derive(Debug)]
pub struct UniFlight<K, T> {
    mapping: SharedMapping<K, T>,
    max_leaders: usize,
}

impl<K, T> Default for UniFlight<K, T> {
    fn default() -> Self {
        Self {
            mapping: Arc::default(),
            max_leaders: 1,
        }
    }
}

struct Shared<T> {
    slot: AsyncMutex<Option<T>>,
    leader_count: AtomicUsize,
    max_leaders: usize,
}

impl<T> Shared<T> {
    fn new(max_leaders: usize) -> Self {
        Self {
            slot: AsyncMutex::new(None),
            leader_count: AtomicUsize::new(0),
            max_leaders,
        }
    }
}

/// RAII guard that decrements leader count on drop.
struct LeaderGuard<T> {
    shared: Option<Arc<Shared<T>>>,
}

impl<T> LeaderGuard<T> {
    /// Try to claim a leader slot. Returns `Some(guard)` if successful, `None` if max leaders reached.
    fn try_claim(shared: &Arc<Shared<T>>) -> Option<Self> {
        let current = shared.leader_count.load(Ordering::Acquire);
        if current < shared.max_leaders {
            let prev = shared.leader_count.fetch_add(1, Ordering::AcqRel);
            if prev < shared.max_leaders {
                return Some(Self {
                    shared: Some(Arc::clone(shared)),
                });
            }
            // Race lost - another caller claimed the last slot
            shared.leader_count.fetch_sub(1, Ordering::AcqRel);
        }
        None
    }

    /// Consume the guard without decrementing (called when leader successfully stores result).
    fn disarm(mut self) -> Arc<Shared<T>> {
        self.shared.take().expect("LeaderGuard shared already taken")
    }
}

impl<T> Drop for LeaderGuard<T> {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.leader_count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// `BroadcastOnce` consists of shared slot and notify.
#[derive(Clone)]
struct BroadcastOnce<T> {
    shared: Weak<Shared<T>>,
}

impl<T> BroadcastOnce<T> {
    fn new(max_leaders: usize) -> (Self, Arc<Shared<T>>) {
        let shared = Arc::new(Shared::new(max_leaders));
        (
            Self {
                shared: Arc::downgrade(&shared),
            },
            shared,
        )
    }
}

/// Role of a caller in the work execution.
enum Role<T, F> {
    /// Leader executes the work closure.
    Leader { func: F, guard: LeaderGuard<T> },
    /// Follower waits for any leader's result. Keeps func for potential promotion.
    Follower { func: F },
}

// After calling BroadcastOnce::waiter we can get a waiter.
// It's in WaitList.
struct BroadcastOnceWaiter<K, T, F> {
    role: Role<T, F>,
    shared: Arc<Shared<T>>,

    key: K,
    mapping: SharedMapping<K, T>,
}

impl<T> std::fmt::Debug for BroadcastOnce<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BroadcastOnce")
    }
}

#[expect(
    clippy::type_complexity,
    reason = "The Result type is complex but intentionally groups related items for the retry pattern"
)]
impl<T> BroadcastOnce<T> {
    /// Attempts to create a waiter for an existing broadcast.
    ///
    /// Returns `Ok` with a waiter (either leader or follower role) if the broadcast is still active.
    /// Returns `Err` if all leaders have dropped (weak reference upgrade failed).
    fn try_waiter<K, F>(
        &self,
        func: F,
        key: K,
        mapping: SharedMapping<K, T>,
    ) -> Result<BroadcastOnceWaiter<K, T, F>, (F, K, SharedMapping<K, T>)> {
        let Some(shared) = self.shared.upgrade() else {
            return Err((func, key, mapping));
        };

        // Try to become a leader if slots are available
        if let Some(guard) = LeaderGuard::try_claim(&shared) {
            return Ok(BroadcastOnceWaiter {
                role: Role::Leader { func, guard },
                shared,
                key,
                mapping,
            });
        }

        // Become a follower (keep func for potential promotion)
        Ok(BroadcastOnceWaiter {
            role: Role::Follower { func },
            shared,
            key,
            mapping,
        })
    }

    /// Creates a waiter for a new broadcast entry (first caller always becomes leader).
    fn leader_waiter<K, F>(shared: Arc<Shared<T>>, func: F, key: K, mapping: SharedMapping<K, T>) -> BroadcastOnceWaiter<K, T, F> {
        // Safe to unwrap: new Shared starts at 0, max_leaders >= 1
        let guard = LeaderGuard::try_claim(&shared).expect("first leader claim should always succeed");
        BroadcastOnceWaiter {
            role: Role::Leader { func, guard },
            shared,
            key,
            mapping,
        }
    }
}

// We already in WaitList, so wait will be fine, we won't miss
// anything after Waiter generated.
impl<K, T, F, Fut> BroadcastOnceWaiter<K, T, F>
where
    K: Hash + Eq,
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
    T: Clone,
{
    async fn wait(self) -> T {
        let Self {
            role,
            shared,
            key,
            mapping,
        } = self;
        match role {
            Role::Leader { func, guard } => Self::wait_as_leader(shared, key, mapping, func, guard).await,
            Role::Follower { func } => Self::wait_as_follower(shared, key, mapping, func).await,
        }
    }

    async fn wait_as_leader(shared: Arc<Shared<T>>, key: K, mapping: SharedMapping<K, T>, func: F, guard: LeaderGuard<T>) -> T {
        // Lock the slot first - this ensures followers wait while we execute
        let mut slot = shared.slot.lock().await;

        // Check if another leader already stored a result
        if let Some(value) = slot.as_ref() {
            let result = value.clone();
            drop(slot);
            guard.disarm();
            return result;
        }

        // Execute the work while holding the lock
        // This ensures followers block on lock().await until we're done
        let value = func().await;
        *slot = Some(value.clone());
        drop(slot);

        // Clean up the mapping entry
        mapping.lock().remove(&key);

        // Disarm the guard (result is stored, count doesn't matter)
        guard.disarm();
        value
    }

    async fn wait_as_follower(shared: Arc<Shared<T>>, key: K, mapping: SharedMapping<K, T>, func: F) -> T {
        // Wait for a result by acquiring the slot lock
        // Leaders hold this lock during execution, so we'll block until one finishes
        let slot = shared.slot.lock().await;
        if let Some(value) = slot.as_ref() {
            return value.clone();
        }
        drop(slot);

        // No result and we acquired the lock - all leaders must have failed
        // Promote ourselves to leader and execute
        // Safe to unwrap: if we got here, leader_count == 0, and max_leaders >= 1
        let guard = LeaderGuard::try_claim(&shared).expect("follower promotion should always succeed");
        Self::wait_as_leader(shared, key, mapping, func, guard).await
    }
}

impl<K, T> UniFlight<K, T>
where
    K: Hash + Eq + Clone,
{
    /// Creates a new `UniFlight` instance with single-leader behavior.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new `UniFlight` instance allowing up to `max_leaders` concurrent executions.
    ///
    /// When multiple tasks request the same work concurrently, up to `max_leaders` of them
    /// will execute in parallel. The first to complete wins, and all other tasks (both
    /// executing leaders and waiting followers) receive that result.
    ///
    /// This is useful for redundancy scenarios where you want multiple attempts at the
    /// same operation and want to use whichever completes first.
    ///
    /// # Panics
    ///
    /// Panics if `max_leaders` is 0.
    ///
    /// # Example
    ///
    /// ```
    /// use uniflight::UniFlight;
    ///
    /// # async fn example() {
    /// // Allow 3 concurrent leaders for redundancy
    /// let group: UniFlight<&str, String> = UniFlight::with_max_leaders(3);
    ///
    /// // Up to 3 concurrent calls will execute in parallel
    /// let result = group.work("key", || async {
    ///     "result".to_string()
    /// }).await;
    /// # }
    /// ```
    #[inline]
    #[must_use]
    pub fn with_max_leaders(max_leaders: usize) -> Self {
        assert!(max_leaders > 0, "max_leaders must be at least 1");
        Self {
            mapping: Arc::default(),
            max_leaders,
        }
    }

    /// Execute and return the value for a given function, making sure that only up to
    /// `max_leaders` operations are in-flight at a given moment. If a duplicate call comes in
    /// beyond the limit, that caller will wait until one of the leaders completes and return
    /// the same value.
    pub fn work<F, Fut>(&self, key: K, func: F) -> impl Future<Output = T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
        T: Clone,
    {
        let owned_mapping = Arc::clone(&self.mapping);
        let mut mapping = self.mapping.lock();
        let val = mapping.get_mut(&key);
        if let Some(call) = val {
            let (func, key, owned_mapping) = match call.try_waiter(func, key, owned_mapping) {
                Ok(waiter) => return waiter.wait(),
                Err(fm) => fm,
            };
            // All leaders dropped - create new broadcast entry
            let (new_call, shared) = BroadcastOnce::new(self.max_leaders);
            *call = new_call;
            let waiter = BroadcastOnce::leader_waiter(shared, func, key, owned_mapping);
            waiter.wait()
        } else {
            // New key - create broadcast entry and become first leader
            let (call, shared) = BroadcastOnce::new(self.max_leaders);
            mapping.insert(key.clone(), call);
            let waiter = BroadcastOnce::leader_waiter(shared, func, key, owned_mapping);
            waiter.wait()
        }
    }
}
