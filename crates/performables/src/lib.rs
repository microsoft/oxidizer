// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![warn(missing_docs)]

//! Performance-oriented ownership and asynchronous synchronization primitives.
//!
//! The synchronization types are executor-independent and optimize for the
//! uncontended case. Acquiring an available lock performs only atomic
//! operations; waiter allocation and queue locking occur after contention is
//! observed. Locks support asynchronous acquisition as well as explicit
//! `lock_sync`, `read_sync`, and `write_sync` operations for synchronous call
//! sites. Default lock acquisition panics on poison, while explicit `*_result`
//! APIs return [`sync::PoisonError`] with the acquired guard for recovery.
//! [`sync::barrier::Barrier`] and [`sync::condition::Condvar`] provide
//! asynchronous and blocking waits, while [`sync::once::OnceLock`] and
//! [`sync::once::LazyLock`] instrument one-time initialization.
//! [`sync::channel`] provides multi-producer queues, oneshot transfer, and
//! independently versioned latest-value observation. The default `seismograph`
//! feature enables runtime ownership and synchronization telemetry.
//!
//! [`arc::Arc`] defaults to a process-wide allocation with the same
//! representation size as [`std::sync::Arc`]. Its thread-aware per-core and
//! per-NUMA strategies lazily materialize and reuse affinity-local values.

pub mod arc;
pub mod sync;

mod telemetry;
