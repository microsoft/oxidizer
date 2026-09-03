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
//! ## Thread-aware isolated runtimes
//!
//! In runtimes that keep work on stable worker threads, each worker should own an
//! isolated clock with its own timer storage. This eliminates cross-thread lock
//! contention.
//!
//! The pattern is to create the root [`InactiveClock`] on its source thread, then
//! move each clone to a worker, build the destination coordinate on that live
//! worker, relocate from the source coordinate, and activate:
//!
//! ```rust
//! # use thread_aware::ThreadAware;
//! # use thread_aware::thread::ThreadBuilder;
//! # use tick::runtime::InactiveClock;
//! # use std::sync::{Arc, Barrier};
//! let builder = ThreadBuilder::default();
//! let source = builder.build(std::thread::current().id());
//! let root = InactiveClock::default();
//!
//! let ready = Arc::new(Barrier::new(3));
//! let first = {
//!     let ready = Arc::clone(&ready);
//!     let builder = builder.clone().numa_node(0);
//!     let source = source.clone();
//!     let mut inactive = root.clone();
//!     std::thread::spawn(move || {
//!         let destination = builder.build(std::thread::current().id());
//!         ready.wait();
//!         inactive.relocate(Some(&source), &destination);
//!
//!         // Keep both values on this worker in a real runtime loop.
//!         let (_clock, _driver) = inactive.activate();
//!     })
//! };
//! let second = {
//!     let ready = Arc::clone(&ready);
//!     let builder = builder.numa_node(1);
//!     let mut inactive = root;
//!     std::thread::spawn(move || {
//!         let destination = builder.build(std::thread::current().id());
//!         ready.wait();
//!         inactive.relocate(Some(&source), &destination);
//!
//!         // Keep both values on this worker in a real runtime loop.
//!         let (_clock, _driver) = inactive.activate();
//!     })
//! };
//!
//! // Both destination coordinates exist on live workers before relocation.
//! ready.wait();
//! first.join().unwrap();
//! second.join().unwrap();
//! ```
//!
//! After relocation, each thread's clock and driver operate on an independent set of timers.
//! Timers registered on `clock_1` are only visible to `driver_1`, and the other way around. Each driver
//! must be advanced independently by its owning thread.
//!
//! ## Work-stealing runtimes
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
//! [`ThreadAware::relocate`]: thread_aware::ThreadAware::relocate

mod clock_driver;
mod clock_gone;
mod inactive_clock;

pub use clock_driver::ClockDriver;
pub use clock_gone::ClockGone;
pub use inactive_clock::{InactiveClock, Isolated, Shared};
