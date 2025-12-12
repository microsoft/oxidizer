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

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{
            AtomicUsize,
            Ordering::{AcqRel, Acquire},
        },
        time::Duration,
    };

    use futures_util::{StreamExt, stream::FuturesUnordered};

    use super::*;

    fn unreachable_future() -> std::future::Pending<String> {
        std::future::pending()
    }

    #[tokio::test]
    async fn direct_call() {
        let group = UniFlight::new();
        let result = group
            .work("key", || async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "Result".to_string()
            })
            .await;
        assert_eq!(result, "Result");
    }

    #[tokio::test]
    async fn parallel_call() {
        let call_counter = AtomicUsize::default();

        let group = UniFlight::new();
        let futures = FuturesUnordered::new();
        for _ in 0..10 {
            futures.push(group.work("key", || async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                call_counter.fetch_add(1, AcqRel);
                "Result".to_string()
            }));
        }

        assert!(futures.all(|out| async move { out == "Result" }).await);
        assert_eq!(call_counter.load(Acquire), 1);
    }

    #[tokio::test]
    async fn parallel_call_seq_await() {
        let call_counter = AtomicUsize::default();

        let group = UniFlight::new();
        let mut futures = Vec::new();
        for _ in 0..10 {
            futures.push(group.work("key", || async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                call_counter.fetch_add(1, AcqRel);
                "Result".to_string()
            }));
        }

        for fut in futures {
            assert_eq!(fut.await, "Result");
        }
        assert_eq!(call_counter.load(Acquire), 1);
    }

    #[tokio::test]
    async fn call_with_static_str_key() {
        let group = UniFlight::new();
        let result = group
            .work("key".to_string(), || async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                "Result".to_string()
            })
            .await;
        assert_eq!(result, "Result");
    }

    #[tokio::test]
    async fn call_with_static_string_key() {
        let group = UniFlight::new();
        let result = group
            .work("key".to_string(), || async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                "Result".to_string()
            })
            .await;
        assert_eq!(result, "Result");
    }

    #[tokio::test]
    async fn call_with_custom_key() {
        #[derive(Clone, PartialEq, Eq, Hash)]
        struct K(i32);
        let group = UniFlight::new();
        let result = group
            .work(K(1), || async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                "Result".to_string()
            })
            .await;
        assert_eq!(result, "Result");
    }

    #[tokio::test]
    async fn late_wait() {
        let group = UniFlight::new();
        let fut_early = group.work("key".to_string(), || async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            "Result".to_string()
        });
        let fut_late = group.work("key".into(), unreachable_future);
        assert_eq!(fut_early.await, "Result");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fut_late.await, "Result");
    }

    #[tokio::test]
    async fn cancel() {
        let group = UniFlight::new();

        // the executer cancelled and the other awaiter will create a new future and execute.
        let fut_cancel = group.work("key".to_string(), unreachable_future);
        let _ = tokio::time::timeout(Duration::from_millis(10), fut_cancel).await;
        let fut_late = group.work("key".to_string(), || async { "Result2".to_string() });
        assert_eq!(fut_late.await, "Result2");

        // the first executer is slow but not dropped, so the result will be the first ones.
        let begin = tokio::time::Instant::now();
        let fut_1 = group.work("key".to_string(), || async {
            tokio::time::sleep(Duration::from_millis(2000)).await;
            "Result1".to_string()
        });
        let fut_2 = group.work("key".to_string(), unreachable_future);
        let (v1, v2) = tokio::join!(fut_1, fut_2);
        assert_eq!(v1, "Result1");
        assert_eq!(v2, "Result1");
        assert!(begin.elapsed() > Duration::from_millis(1500));
    }

    #[tokio::test]
    async fn leader_panic_in_spawned_task() {
        let call_counter = AtomicUsize::default();
        let group: Arc<UniFlight<String, String>> = Arc::new(UniFlight::new());

        // First task will panic in a spawned task (no catch_unwind)
        let group_clone = Arc::clone(&group);
        let handle = tokio::spawn(async move {
            group_clone
                .work("key".to_string(), || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    panic!("leader panicked in spawned task");
                    #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
                    "never".to_string()
                })
                .await
        });

        // Give time for the spawned task to register and start
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Second task should become the new leader after the first panics
        let group_clone = Arc::clone(&group);
        let call_counter_ref = &call_counter;
        let fut_follower = group_clone.work("key".to_string(), || async {
            call_counter_ref.fetch_add(1, AcqRel);
            "Result".to_string()
        });

        // Wait for the spawned task to panic
        let spawn_result = handle.await;
        assert!(spawn_result.is_err());

        // The follower should succeed - Rust's drop semantics ensure the mutex is released
        let result = fut_follower.await;
        assert_eq!(result, "Result");
        assert_eq!(call_counter.load(Acquire), 1);
    }

    #[tokio::test]
    async fn debug_impl() {
        let group: UniFlight<String, String> = UniFlight::new();

        // Test Debug on empty group
        let debug_str = format!("{:?}", group);
        assert!(debug_str.contains("UniFlight"));

        // Create a pending work item to populate the mapping with a BroadcastOnce
        let fut = group.work("key".to_string(), || async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            "Result".to_string()
        });

        // Debug should still work with entries in the mapping
        let debug_str = format!("{:?}", group);
        assert!(debug_str.contains("UniFlight"));
        assert!(debug_str.contains("BroadcastOnce"));

        // Complete the work
        assert_eq!(fut.await, "Result");
    }
}
