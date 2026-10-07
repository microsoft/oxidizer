// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Clocks, delays, and tests that do not have to wait for real time.
//!
//! Arty re-exports [`tick`] through [`arty::time`](crate::time). The `time`
//! feature works without an Arty runtime; `rt` also enables time and drives
//! timers for its workers.
//!
//! # Use the task's clock
//!
//! Each task gets a clock through [`Builtins::clock`](crate::task::Builtins::clock).
//! Use a stopwatch for elapsed time and `system_time()` for a wall-clock
//! timestamp, which can change independently of elapsed time.
//!
//! ```
//! use std::future::pending;
//! use std::time::Duration;
//!
//! use arty::task::Builtins;
//! use tick::FutureExt;
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) {
//! let watch = cx.clock().stopwatch();
//! cx.clock().delay(Duration::from_millis(1)).await;
//! assert!(watch.elapsed() >= Duration::from_millis(1));
//!
//! let result = pending::<()>()
//!     .timeout(cx.clock(), Duration::from_millis(1))
//!     .await;
//! assert!(result.is_err());
//! # }
//! ```
//!
//! A busy worker may resume a task later than the requested delay. A timeout
//! limits how long you wait for a result; it does not cancel a spawned task.
//!
//! # Control time in tests
//!
//! Enable `test-util` in an Arty dev-dependency. A second
//! [`ClockControl`](crate::time::ClockControl) parameter to
//! [`arty::test`](crate::test) gives the test control over its runtime's clocks.
//! Time starts at the UNIX epoch and does not advance automatically.
//!
//! A delay starts its timer when first polled. Poll it before advancing time:
//!
//! ```test_harness
//! use std::future::{Future, poll_fn};
//! use std::pin::pin;
//! use std::task::Poll;
//! use std::time::Duration;
//!
//! use arty::task::Builtins;
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
//! `auto_advance_timers(true)` is convenient for one delay after another, but
//! it advances eagerly and does not guarantee concurrent timers finish in order.
//! Advance time explicitly when ordering matters. `auto_advance(duration)`
//! instead advances time on reads.
//!
//! `ClockControl` can be combined with `workers`. For a custom runtime builder,
//! create the control in a synchronous test and pass a clone to
//! [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock).
//!
//! # Without an Arty runtime
//!
//! Enabling `time` alone does not start a timer driver.
//! The underlying `tick::SimpleClock` reads time without a driver, but delays
//! on [`Clock`](crate::time::Clock) need a driver or controlled time. See
//! [`tick`] for using these clocks outside Arty.
