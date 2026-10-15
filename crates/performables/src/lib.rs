// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

//! Performance-oriented ownership and synchronization primitives.
//!
//! The synchronization types are executor-independent and optimize for the
//! uncontended case. Acquiring an available lock does not allocate; async waiter
//! storage is allocated lazily after contention is observed and reused
//! thereafter. Mutexes, reader-writer locks, barriers, and condition variables
//! default to compact [`sync::mode::Sync`] storage. Select [`sync::mode::Async`]
//! when the same object must support asynchronous waits as well as blocking
//! callers. Synchronous operations use unsuffixed names such as
//! [`lock`](sync::mutex::Mutex::lock), while asynchronous operations are explicit,
//! such as [`lock_async`](sync::mutex::Mutex::lock_async). This convention also
//! applies to reader-writer locks, waits, and channels.
//! Default lock acquisition panics on poison, while explicit `*_result`
//! APIs return [`sync::PoisonError`] with the acquired guard for recovery.
//! [`sync::barrier::Barrier`] and [`sync::condition::Condvar`] follow the same
//! mode selection, while [`sync::once::OnceLock`] and
//! [`sync::once::LazyLock`] instrument one-time initialization.
//! [`sync::channel`] provides multi-producer queues, oneshot transfer, and
//! independently versioned latest-value observation. The optional `seismograph`
//! feature enables runtime ownership and synchronization telemetry.
//!
//! [`arc::Arc`] defaults to a process-wide allocation with the same
//! representation size as [`std::sync::Arc`]. Its thread-aware per-thread and
//! per-NUMA strategies lazily materialize and reuse affinity-local values.
//!
//! # Choosing a synchronization mode
//!
//! The mode belongs to the shared object, not to individual callers.
//! Blocking and asynchronous acquisitions of an async-capable lock coordinate
//! through the same ownership state. Synchronous guards follow the native
//! backend's thread-affinity requirements and must not be held across an await.
//! Mode selection adds no identity field: telemetry continues to identify each
//! lock, barrier, or condition variable by its outer object's address, not its
//! backend or lazily allocated wait queue. As before, moving an object changes
//! that identity, and an address can be reused after an object is dropped.
//!
//! ```
//! use performables::sync::mode;
//! use performables::sync::mutex::Mutex;
//!
//! let counter = Mutex::<u64>::new(0);
//! *counter.lock() += 1;
//! assert_eq!(*counter.lock(), 1);
//!
//! let shared = Mutex::<u64, mode::Async>::new(0);
//! *shared.lock() += 1;
//! let acquire = shared.lock_async();
//! // A pending acquisition does not take ownership until it is polled.
//! drop(acquire);
//! assert_eq!(*shared.lock(), 1);
//! ```
//!
//! Async methods are intentionally unavailable on synchronous objects:
//!
//! ```compile_fail
//! use performables::sync::mutex::Mutex;
//! let mutex = Mutex::<u64>::new(0);
//! let _future = mutex.lock_async();
//! ```
//!
//! ```compile_fail
//! use performables::sync::lock::RwLock;
//! let lock = RwLock::<u64>::new(0);
//! let _future = lock.read_async();
//! ```
//!
//! ```compile_fail
//! use performables::sync::{condition::Condvar, mode, mutex::Mutex};
//! let mutex = Mutex::<u64>::new(0);
//! let condition = Condvar::<mode::Sync>::new();
//! let _future = condition.wait_async(mutex.lock());
//! ```
//!
//! ```compile_fail
//! use performables::sync::{barrier::Barrier, mode};
//! let barrier = Barrier::<mode::Sync>::new(1);
//! let _future = barrier.wait_async();
//! ```
//!
//! Native synchronous guards cannot be transferred to another thread:
//!
//! ```compile_fail
//! use performables::sync::mutex::Mutex;
//! fn require_send<T: Send>(_: T) {}
//! let mutex = Mutex::<u64>::new(0);
//! require_send(mutex.lock());
//! ```

pub mod arc;
pub mod sync;

mod telemetry;
