// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

//! Shared ownership and synchronization with thread-aware runtime and
//! `seismograph` integration.
//!
//! `performables` provides [`Arc`](arc::Arc), mutex and reader-writer locks,
//! barriers, condition variables, retained notifications, one-time initialization,
//! and channels.
//! Synchronization supports blocking callers and executor-independent async
//! waits.
//!
//! [`sync::notify::Notify`] retains one coalesced notification for a later wait,
//! with blocking and executor-independent async support through the same modes.
//! Ready blocking channel operations do not allocate waiter storage. Queue
//! payload storage and enabled telemetry may allocate independently. Final
//! arrivals at an async-capable barrier do not allocate their own waiter storage;
//! releasing other participants can still execute their wakers.
//!
//! With recording disabled, watch registration and cancellation reuse their
//! reserved waiter slots after warm-up, up to the retained capacity. A
//! single-participant barrier wait and condition-variable notifications with no
//! waiters also allocate no primitive-owned storage on the owned Windows/Linux
//! backends. These properties exclude construction, payload allocation and
//! user-provided waker behavior; they do not promise stable object sizes or a
//! particular memory layout.
//!
//! # Why use this instead of `std` or `tokio`?
//!
//! The crate offers:
//!
//! - **Uncontended performance.** Optimized for uncontended access, which is
//!   common in thread-per-core environments.
//! - **`seismograph` telemetry.** The optional `seismograph` feature records
//!   ownership, lock contention, and channel activity through Seismograph's
//!   recorder. Normal operations supply the events without manual
//!   instrumentation at each call site.
//! - **Thread-aware ownership.** `Arc` with [`PerThread`](arc::PerThread) or
//!   [`PerNuma`](arc::PerNuma) implements
//!   [`ThreadAware`](thread_aware_core::ThreadAware) when its pointee is
//!   `Send + Sync`. A thread-aware runtime can relocate the pointer to a value
//!   local to the destination thread or NUMA node, rather than sharing one
//!   process-wide value as `std::sync::Arc` does.
//!
//! See [`arc`] and [`sync`] for the available types and their full APIs.

pub mod arc;
pub mod sync;

mod error;
mod telemetry;

pub use error::Error;
