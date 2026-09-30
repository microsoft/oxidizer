// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker-driven clocks, timeouts, and controlled time in tests.
//!
//! Arty re-exports its time primitives from [`tick`] through
//! [`arty::time`](crate::time). Enable `time` to use them independently, or
//! `rt`, which implies `time`, to have Arty drive worker timers.
//!
//! # Use the task's clock
//!
//! [`Builtins::clock`](crate::runtime::Builtins::clock) provides the clock
//! associated with the task's worker. Use a stopwatch for elapsed time and
//! `system_time()` when an absolute wall-clock timestamp is needed. Wall-clock
//! time can change independently of monotonic elapsed time.
//!
//! ```
//! use std::future::pending;
//! use std::time::Duration;
//!
//! use arty::runtime::Builtins;
//! use arty::time::FutureExt;
//!
//! #[arty::main]
//! async fn main(cx: Builtins) {
//!     let watch = cx.clock().stopwatch();
//!     cx.clock().delay(Duration::from_millis(1)).await;
//!     assert!(watch.elapsed() >= Duration::from_millis(1));
//!
//!     let result = pending::<()>()
//!         .timeout(cx.clock(), Duration::from_millis(1))
//!         .await;
//!     assert!(result.is_err());
//! }
//! ```
//!
//! A requested delay is not a deadline for the worker to resume the task;
//! scheduling load and host timer granularity affect when it runs again.
//! Applying a timeout to a join bounds the wait for its result while the clock
//! is driven. It does not cancel the spawned task: dropping a join does not
//! request cancellation. A stopped runtime cannot supply an independent timer
//! for waiting on its own shutdown.
//!
//! # Control time in tests
//!
//! Enable `test-util` in an Arty **dev-dependency** to use `ClockControl`.
//! A second owned `ClockControl` parameter to [`arty::test`](crate::test)
//! creates a fresh control and connects it to the runtime's clocks. It starts
//! at the UNIX epoch with automatic advancement disabled. Clones share one
//! time domain. Enabling `test-util` alone does not change the clock used by
//! a test taking only `Builtins`.
//!
//! A delay registers its timer when first polled, not when constructed.
//! Poll it before advancing time manually:
//!
//! ```test_harness
//! use std::future::{Future, poll_fn};
//! use std::pin::pin;
//! use std::task::Poll;
//! use std::time::Duration;
//!
//! use arty::runtime::Builtins;
//! use arty::time::ClockControl;
//!
//! #[arty::test]
//! async fn controlled_delay(cx: Builtins, control: ClockControl) {
//!     let watch = cx.clock().stopwatch();
//!     let mut delay = pin!(cx.clock().delay(Duration::from_secs(30)));
//!     poll_fn(|context| {
//!         assert!(delay.as_mut().poll(context).is_pending());
//!         Poll::Ready(())
//!     })
//!     .await;
//!     control.advance(Duration::from_secs(30));
//!     assert_eq!(watch.elapsed(), Duration::from_secs(30));
//!     delay.await;
//! }
//! ```
//!
//! `auto_advance(duration)` advances time on reads.
//! `auto_advance_timers(true)` advances eagerly when timers are registered or
//! time is read. It is convenient for sequential delays, but is **not**
//! Tokio's idle-runtime advancement policy and does not simulate concurrent
//! timers completing in deadline order. Use explicit advancement when the
//! relative ordering of timers matters.
//!
//! The clock-control parameter can accompany `workers`, but not `builder`.
//! To combine custom configuration with controlled time, use an ordinary
//! synchronous test: create a control, pass a clone to
//! [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock), build the
//! runtime, and run the asynchronous body. The [`test`](crate::test) attribute
//! reference describes the complete syntax.
//!
//! # Runtime-independent time
//!
//! Enabling `time` alone does not start workers or a timer driver. `SimpleClock`
//! supplies time queries without timers; a `Clock` used for delays needs a
//! driver or controlled time. See [`tick`]'s clock documentation for integration
//! outside Arty rather than assuming a timer will advance by itself.
