// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Infrastructure for integrating time primitives into async runtimes.
//!
//! This module provides the necessary components to bridge time-based operations
//! with async runtime execution. The primary workflow involves:
//!
//! 1. Start with an [`InactiveClock`] that can be safely moved across threads
//! 2. Activate it using [`InactiveClock::activate`] to get a [`Clock`] and [`ClockDriver`]
//! 3. Use the [`ClockDriver`] to periodically advance timers in your runtime loop
//! 4. Use the [`Clock`] for time operations like creating timers and measuring time
//!
//! # Integration with Runtimes
//!
//! Different runtime architectures can integrate this module as follows:
//!
//! ## Thread-per-core Runtimes
//!
//! In thread-per-core architectures, each thread should own an isolated clock with its own
//! timer storage. This eliminates cross-thread lock contention and provides linear scalability.
//!
//! The pattern is to clone the [`InactiveClock`], relocate each clone to its target thread
//! using [`ThreadAware::relocated`], and then activate:
//!
//! ```rust
//! # use thread_aware::ThreadAware;
//! # use thread_aware::affinity::pinned_affinities;
//! # use tick::runtime::InactiveClock;
//! # let affinities = pinned_affinities(&[1, 1]);
//! let root = InactiveClock::default();
//!
//! // Clone and relocate to each thread's affinity
//! let inactive_1 = root.clone().relocated(affinities[0].into(), affinities[0]);
//! let inactive_2 = root.relocated(affinities[1].into(), affinities[1]);
//!
//! // On thread 1: activate and drive timers independently
//! let (clock_1, driver_1) = inactive_1.activate();
//!
//! // On thread 2: activate and drive timers independently
//! let (clock_2, driver_2) = inactive_2.activate();
//! ```
//!
//! After relocation, each thread's clock and driver operate on an independent set of timers.
//! Timers registered on `clock_1` are only visible to `driver_1`, and vice versa. Each driver
//! must be advanced independently by its owning thread.
//!
//! ## Multi-threaded Runtimes
//!
//! In multi-threaded runtimes where tasks may run on any thread, activate once and share the
//! clock across threads. The driver should be kept on a dedicated thread or task for timer
//! advancement:
//!
//! ```rust
//! # use tick::runtime::InactiveClock;
//! let (clock, driver) = InactiveClock::default().activate();
//!
//! // Share `clock` across threads (it is Clone + Send + Sync)
//! // Keep `driver` on a single thread to advance timers
//! ```
//!
//! [`Clock`]: crate::Clock
//! [`InactiveClock::activate`]: InactiveClock::activate
//! [`ThreadAware::relocated`]: thread_aware::ThreadAware::relocated

mod clock_driver;
mod clock_gone;
mod inactive_clock;

pub use clock_driver::ClockDriver;
pub use clock_gone::ClockGone;
pub use inactive_clock::InactiveClock;
