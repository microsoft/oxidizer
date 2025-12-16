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

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/uniflight/favicon.ico")]

use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Weak},
};

use parking_lot::Mutex as SyncMutex;
use xutex::AsyncMutex;

type SharedMapping<K, T> = Arc<SyncMutex<HashMap<K, BroadcastOnce<T>>>>;

/// Represents a class of work and creates a space in which units of work
/// can be executed with duplicate suppression.
#[derive(Debug)]
pub struct UniFlight<K, T> {
    mapping: SharedMapping<K, T>,
}

impl<K, T> Default for UniFlight<K, T> {
    fn default() -> Self {
        Self { mapping: Arc::default() }
    }
}

struct Shared<T> {
    slot: AsyncMutex<Option<T>>,
}

impl<T> Default for Shared<T> {
    fn default() -> Self {
        Self {
            slot: AsyncMutex::new(None),
        }
    }
}

/// `BroadcastOnce` consists of shared slot and notify.
#[derive(Clone)]
struct BroadcastOnce<T> {
    shared: Weak<Shared<T>>,
}

impl<T> BroadcastOnce<T> {
    fn new() -> (Self, Arc<Shared<T>>) {
        let shared = Arc::new(Shared::default());
        (
            Self {
                shared: Arc::downgrade(&shared),
            },
            shared,
        )
    }
}

// After calling BroadcastOnce::waiter we can get a waiter.
// It's in WaitList.
struct BroadcastOnceWaiter<K, T, F> {
    func: F,
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
    fn try_waiter<K, F>(
        &self,
        func: F,
        key: K,
        mapping: SharedMapping<K, T>,
    ) -> Result<BroadcastOnceWaiter<K, T, F>, (F, K, SharedMapping<K, T>)> {
        let Some(upgraded) = self.shared.upgrade() else {
            return Err((func, key, mapping));
        };
        Ok(BroadcastOnceWaiter {
            func,
            shared: upgraded,
            key,
            mapping,
        })
    }

    #[inline]
    const fn waiter<K, F>(shared: Arc<Shared<T>>, func: F, key: K, mapping: SharedMapping<K, T>) -> BroadcastOnceWaiter<K, T, F> {
        BroadcastOnceWaiter {
            func,
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
        let mut slot = self.shared.slot.lock().await;
        if let Some(value) = (*slot).as_ref() {
            return value.clone();
        }

        let value = (self.func)().await;
        *slot = Some(value.clone());

        self.mapping.lock().remove(&self.key);

        value
    }
}

impl<K, T> UniFlight<K, T>
where
    K: Hash + Eq + Clone,
{
    /// Creates a new `UniFlight` instance.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute and return the value for a given function, making sure that only one
    /// operation is in-flight at a given moment. If a duplicate call comes in, that caller will
    /// wait until the original call completes and return the same value.
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
            let (new_call, shared) = BroadcastOnce::new();
            *call = new_call;
            let waiter = BroadcastOnce::waiter(shared, func, key, owned_mapping);
            waiter.wait()
        } else {
            let (call, shared) = BroadcastOnce::new();
            mapping.insert(key.clone(), call);
            let waiter = BroadcastOnce::waiter(shared, func, key, owned_mapping);
            waiter.wait()
        }
    }
}
