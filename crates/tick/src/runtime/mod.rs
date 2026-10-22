// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Components that connect time operations to async runtimes.
//!
//! Use this workflow:
//!
//! 1. Create an [`InactiveClock`]. You can move it across threads.
//! 2. Activate it with the runtime driver waker. Activation returns a [`Clock`]
//!    and [`ClockDriver`].
//! 3. Advance the driver. Park until its next deadline or notification.
//! 4. Use the clock to create timers and measure time.
//!
//! # Runtime integration
//!
//! Different runtime architectures can integrate this module as follows:
//!
//! ## Thread-aware isolated runtimes
//!
//! If a runtime keeps work on stable worker threads, give each worker an
//! isolated clock. Each clock then has separate timer storage. This removes
//! timer-lock contention between workers.
//!
//! Create the root [`InactiveClock`] on its source thread. Then complete these
//! steps for each clone:
//!
//! 1. Move the clone to a worker.
//! 2. Build the destination coordinate on that worker.
//! 3. Relocate the clone from the source coordinate.
//! 4. Activate the clone.
//!
//! ```rust
//! # use thread_aware::{ThreadAware, ThreadBuilder};
//! # use tick::runtime::InactiveClock;
//! # use std::sync::{Arc, Barrier};
//! # use std::task::{Wake, Waker};
//! # struct DriverWake(std::thread::Thread);
//! # impl Wake for DriverWake {
//! #     fn wake(self: Arc<Self>) { self.0.unpark(); }
//! #     fn wake_by_ref(self: &Arc<Self>) { self.0.unpark(); }
//! # }
//! let builder = ThreadBuilder::default();
//! let source = builder.build(std::thread::current().id());
//! let root = InactiveClock::default();
//!
//! let ready = Arc::new(Barrier::new(3));
//! let first = {
//!     let ready = Arc::clone(&ready);
//!     let builder = builder.clone().with_numa_node(0);
//!     let source = source.clone();
//!     let mut inactive = root.clone();
//!     std::thread::spawn(move || {
//!         let destination = builder.build(std::thread::current().id());
//!         ready.wait();
//!         inactive.relocate(Some(&source), &destination);
//!
//!         // Keep both values on this worker in a real runtime loop.
//!         let driver_waker = Waker::from(Arc::new(DriverWake(std::thread::current())));
//!         let (_clock, _driver) = inactive.activate(driver_waker);
//!     })
//! };
//! let second = {
//!     let ready = Arc::clone(&ready);
//!     let builder = builder.with_numa_node(1);
//!     let mut inactive = root;
//!     std::thread::spawn(move || {
//!         let destination = builder.build(std::thread::current().id());
//!         ready.wait();
//!         inactive.relocate(Some(&source), &destination);
//!
//!         // Keep both values on this worker in a real runtime loop.
//!         let driver_waker = Waker::from(Arc::new(DriverWake(std::thread::current())));
//!         let (_clock, _driver) = inactive.activate(driver_waker);
//!     })
//! };
//!
//! // Both destination coordinates exist on live workers before relocation.
//! ready.wait();
//! first.join().unwrap();
//! second.join().unwrap();
//! ```
//!
//! The coordinates come from clones of one `ThreadBuilder`, so they share one
//! runtime owner. Each thread therefore has an independent timer set. A timer
//! is visible only to the driver on its thread. Each thread must advance its
//! own driver.
//!
//! A relocation to another owner keeps the existing timer storage. The clock
//! still works, but its storage does not become local to the destination.
//!
//! ## Work-stealing runtimes
//!
//! If tasks can run on any thread, activate the clock once and share it. Keep
//! the driver on one dedicated thread or task.
//!
//! ```rust
//! # use std::sync::Arc;
//! # use std::task::{Wake, Waker};
//! # use tick::runtime::InactiveClock;
//! # struct DriverWake(std::thread::Thread);
//! # impl Wake for DriverWake {
//! #     fn wake(self: Arc<Self>) { self.0.unpark(); }
//! #     fn wake_by_ref(self: &Arc<Self>) { self.0.unpark(); }
//! # }
//! let driver_waker = Waker::from(Arc::new(DriverWake(std::thread::current())));
//! let (clock, driver) = InactiveClock::default().activate(driver_waker);
//!
//! // Share `clock` across threads (it is Clone + Send + Sync)
//! // Keep `driver` on a single thread to advance timers
//! ```
//!
//! ## Controlled clocks
//!
//! The owner of a controlled clock advances time and wakes timers. The clock
//! ignores the activation waker. Its driver has no system parking deadline.
//!
//! During shutdown, external lifecycle state must wake, stop, and drop each
//! controlled driver loop. The loop must not wait only for [`ClockGone`].
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
