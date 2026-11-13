// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(feature = "rt", feature = "macros", feature = "test-util")),
    expect(
        rustdoc::broken_intra_doc_links,
        reason = "feature-gated items are unavailable in partial documentation builds"
    )
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Single-threaded, thread-aware runtime for async applications.
//!
//! Each async task stays on one worker thread for its entire life, even when
//! Arty manages multiple workers. Tasks can keep thread-local and non-[`Send`]
//! state across awaits.
//!
//! Arty can pass values between workers without moving running tasks.
//! The [`ThreadAware`](core::ThreadAware) trait lets a value update its state
//! for the worker that receives it. This update is called relocation.
//!
//! # Quickstart
//!
//! Add Arty with its default runtime and macro features:
//!
//! ```sh
//! cargo add arty
//! ```
//!
//! ```rust
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! use arty::task::{Context, JoinError};
//!
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! #[arty::main(monitor = disabled)]
//! async fn main(cx: Context) -> Result<(), JoinError> {
//!     let answer = cx.scheduler().spawn(async |_| 6 * 7).await?;
//! #   assert_eq!(answer, 42);
//!     println!("{answer}");
//!     Ok(())
//! }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! This prints `42`. [`main`] starts and stops the runtime.
//! The `?` returns a [`JoinError`](crate::task::JoinError) if the child task fails.
//! Await child tasks before returning. Shutdown cancels pending async tasks.
//!
//! [`Scheduler::spawn`](crate::task::Scheduler::spawn) starts a child task on the
//! scheduler's current thread. The scheduler's thread can differ from the thread
//! that calls `spawn`. See [`task`] to learn how to schedule tasks.
//!
//! # Additional documentation
//!
//! - [`runtime`] explains how to create, configure, and stop a runtime.
//! - [`task`] explains how to start tasks, choose their workers, and receive results.
//! - [`time`] provides clocks, delays, timeouts, and control over time in tests.
//! - [`io`] connects I/O libraries to the runtime.
//! - [`core`] describes workers and how values update their state for a worker.
//! - [`main`] and [`test`] start and stop a runtime for an async function or test.
//!
//! # Features
//!
//! The default feature set enables `rt`, `macros`, and `monitor`.
//!
//! - `rt`: Enables the runtime, task, and I/O APIs.
//! - `macros`: Enables [`main`] and [`test`]. It also enables `rt`.
//! - `monitor`: Connects runtime workers and tasks to Seismograph. An enabled
//!   [`MonitorPolicy`](crate::runtime::MonitorPolicy) starts a local connection for
//!   the Seismograph CLI. Task collection runs on request. Event recording stays
//!   off until an application or the CLI enables it. This feature also forwards
//!   monitoring support for `performables` when `performables` is enabled.
//! - `test-util`: Enables testing utilities, including
//!   [`ClockControl`](crate::time::ClockControl).
//!   Enable it in dev-dependencies, not production dependencies.

use arty_io_core as _;
/// Runs an async entry point on an Arty runtime.
///
/// Apply it to an `async fn` that takes one owned
/// [`Context`](crate::task::Context) argument. It creates a synchronous
/// function with no arguments and the same visibility and return type.
/// The async body runs on an Arty worker, not on the calling thread. For
/// example, name the argument `cx` or `_cx`. The argument must be an identifier.
/// The attribute does not support `_` or destructuring patterns.
///
/// The runtime stops when the body returns. Await required child tasks first.
/// Shutdown cancels pending async tasks.
///
/// Enable the `macros` feature to use this attribute.
///
/// # Configuration
///
/// Without options, the attribute uses
/// [`Runtime::new`](crate::runtime::Runtime::new). It accepts these options:
///
/// - `workers = N` starts exactly `N` async workers. Use an integer literal
///   that fits `usize` (an explicit `usize` suffix is fine). If fewer processors
///   are available, multiple worker threads share each processor. A value of zero
///   causes runtime construction to fail.
/// - `workers = all` starts one async worker per available processor.
/// - `monitor = enabled` or `monitor = disabled` selects whether the runtime
///   starts a local Seismograph monitor when the `monitor` feature is present.
///   The default is `enabled`.
/// - `panic = shutdown` or `panic = ignore` selects whether the runtime stops
///   after Arty catches a task panic. The default is `shutdown`.
///
/// The worker count does not limit threads that run blocking functions.
/// Without `workers`, the runtime uses
/// [`WorkersPolicy::default`](crate::runtime::WorkersPolicy::default).
/// The default is one worker, for both applications and tests.
/// Each worker uses an operating-system thread and its configured stack.
/// Large counts can exceed operating-system thread or memory limits.
///
/// To configure clocks, blocking pools, stack size, or a maximum worker count,
/// use [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) instead of the attribute.
///
/// # Panics
///
/// Errors while creating or stopping the runtime become panics.
/// Cancellation of the main task also becomes a panic.
/// The attribute stops the runtime before it returns the body's value or
/// continues its original panic on the calling thread.
/// If the main task and shutdown both fail, the attribute reports the main task's failure.
/// If a runtime failure cancels the main task, the panic message includes that failure.
///
/// An application error returned by the body remains its return value. The
/// attribute does not convert it to a panic. Use
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build),
/// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on), and
/// [`Runtime::stop`](crate::runtime::Runtime::stop) to handle runtime errors
/// directly. Starting or preparing worker threads can still panic.
///
/// # Examples
///
/// Wait briefly and print a message:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main(monitor = disabled)]
/// # async fn main(cx: arty::task::Context) {
/// cx.clock().delay(std::time::Duration::from_millis(1)).await;
/// println!("Hello from Arty!");
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
///
/// Disable monitoring for the runtime:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main(monitor = disabled)]
/// async fn main(_cx: arty::task::Context) {}
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
///
/// Keep the runtime running after a task panic that Arty catches:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main(panic = ignore, monitor = disabled)]
/// async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
///     assert!(
///         cx.scheduler()
///             .spawn(async |_| panic!("contained"))
///             .await
///             .unwrap_err()
///             .is_panic()
///     );
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
///     Ok(())
/// }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
///
/// Set the async worker count:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main(workers = 4, monitor = disabled)]
/// async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
///     let answer = cx.scheduler().spawn(async |_| 42).await?;
///     assert_eq!(answer, 42);
///     Ok(())
/// }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[cfg(feature = "macros")]
#[doc(inline)]
pub use arty_macros::main;
/// Runs an async test on an Arty runtime.
///
/// Apply it to an `async fn` taking one owned
/// [`Context`](crate::task::Context) argument. It registers a synchronous
/// test that starts its own runtime, runs the body on a worker, and stops the
/// runtime when the body returns.
/// Name the argument `_cx` if it is unused.
///
/// The test keeps its return value, visibility, and attributes such as
/// `#[should_panic]` and `#[ignore]`. An ignored test starts no runtime unless
/// the test runner runs it.
///
/// Enable `macros` to use this attribute. The [`main`] attribute's `workers`,
/// `monitor`, and `panic` options also apply.
/// By default, a test uses one worker, disables monitoring, and stops its
/// runtime after Arty catches a task panic.
/// To test monitoring, use `monitor = enabled`.
///
/// # Simulated time
///
/// Enable `test-util` in an Arty dev-dependency and add a second owned
/// `arty::time::ClockControl` argument. This gives the test control over its
/// runtime's clocks. Time starts at the UNIX epoch and does not advance
/// automatically. Cloned controls share that time. Use different names for the
/// two arguments. Type aliases and associated types are supported.
///
/// Use `control.advance(duration)` to move time forward. Poll a delay before
/// advancing: it starts its timer on the first poll. For delays that run one
/// after another, `auto_advance_timers(true)` advances time without waiting for
/// the runtime to become idle. It does not guarantee which overlapping timer
/// finishes first.
///
/// The control argument works with `workers`. To use a custom runtime builder
/// with controlled time, create the control in a synchronous test and call
/// `builder.clock(control.clone()).build()`. Enabling `test-util` alone does
/// not change a one-argument test's clock.
///
/// # Panics
///
/// Errors while creating or stopping the runtime become panics.
/// Cancellation of the test body also becomes a panic.
/// The attribute stops the runtime before reporting the test's outcome.
/// If the body panics and shutdown also fails, the attribute continues the
/// body's panic with its original value. Therefore,
/// `#[should_panic(expected = "...")]` can match the original message.
/// If a runtime failure cancels the test body, the panic message includes that
/// failure.
///
/// An application error returned by the body remains its return value. Use
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) when you need
/// to handle runtime errors without converting them to panics.
/// Use [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on)
/// and [`Runtime::stop`](crate::runtime::Runtime::stop) to run and stop the runtime.
///
/// # Examples
///
/// Await a task and inspect its result:
///
/// ```test_harness
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::test]
/// async fn answer(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
///     Ok(())
/// }
/// ```
///
/// Enable monitoring for a test that exercises monitoring behavior:
///
/// ```no_run,test_harness
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::test(monitor = enabled)]
/// async fn monitored(_cx: arty::task::Context) {}
/// ```
///
/// Keep the test runtime running after Arty catches a child task's panic:
///
/// ```test_harness
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::test(panic = ignore)]
/// async fn contained_panic(cx: arty::task::Context) {
///     assert!(cx.scheduler().spawn(async |_| panic!("contained")).await.unwrap_err().is_panic());
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
/// }
/// ```
///
/// Test a sequential delay without waiting for real time:
///
/// ```test_harness
/// # #[cfg(all(feature = "macros", feature = "rt", feature = "test-util"))]
/// #[arty::test]
/// async fn simulated_delay(cx: arty::task::Context, control: arty::time::ClockControl) {
///     use std::time::Duration;
///
///     let control = control.auto_advance_timers(true);
///     let watch = cx.clock().stopwatch();
///     cx.clock().delay(Duration::from_secs(30)).await;
///     assert_eq!(watch.elapsed(), Duration::from_secs(30));
///
///     control.advance(Duration::from_secs(5));
///     assert_eq!(watch.elapsed(), Duration::from_secs(35));
/// }
/// ```
#[cfg(feature = "macros")]
#[doc(inline)]
pub use arty_macros::test;

#[cfg(any(test, feature = "rt"))]
pub mod io;
#[cfg(any(test, feature = "rt"))]
pub mod runtime;
#[cfg(any(test, feature = "rt"))]
pub mod task;

/// Worker details and updates to a value's state for a worker.
///
/// [`Thread`](crate::core::Thread) records a runtime [`Owner`](crate::core::Owner),
/// an operating-system thread, and its [`NumaNode`](crate::core::NumaNode).
/// A NUMA node groups processors near the same memory.
/// `Thread` describes where a value belongs.
/// It does not own that thread or keep its runtime running.
///
/// Relocation updates a value's state for a selected worker.
/// Implement [`ThreadAware`](crate::core::ThreadAware) when a value needs this update.
/// Relocation does not move a running task or change the thread that runs your code.
///
/// # Example
///
/// Read the details of the worker that runs a task. This example requires
/// `macros`, which enables `rt`:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main(monitor = disabled)]
/// # async fn main(cx: arty::task::Context) {
/// let worker: &arty::core::Thread = cx.thread();
/// assert_eq!(worker.id(), std::thread::current().id());
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
pub mod core {
    #[doc(inline)]
    pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
}

/// Clocks, delays, and controlled test time.
///
/// Arty provides [`Clock`](crate::time::Clock) and, with `test-util`,
/// [`ClockControl`](crate::time::ClockControl). Use [`tick`] directly for delays,
/// timeouts, stopwatches, and timers that repeat.
/// It also provides extension traits, which add methods to existing types.
/// Add `tick` as a direct dependency before you use these APIs.
/// `Clock` is always available without an Arty runtime. An Arty runtime drives
/// system timers for its workers and wakes tasks when their timers expire.
///
/// # Use the task clock
///
/// Each task gets a clock through
/// [`Context::clock`](crate::task::Context::clock). Use a stopwatch for elapsed
/// time. Use `system_time()` for the current date and time, which can change
/// independently of elapsed time. This example requires `macros`, which enables
/// `rt`.
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main(monitor = disabled)]
/// # async fn main(cx: arty::task::Context) {
/// use std::future::pending;
/// use std::time::Duration;
///
/// use tick::FutureExt;
///
/// let watch = cx.clock().stopwatch();
/// cx.clock().delay(Duration::from_millis(1)).await;
/// assert!(watch.elapsed() >= Duration::from_millis(1));
///
/// let result = pending::<()>()
///     .timeout(cx.clock(), Duration::from_millis(1))
///     .await;
/// assert!(result.is_err());
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
///
/// A busy worker can resume a task later than the requested delay. A timeout
/// limits how long you wait for a result. It does not cancel a spawned task.
///
/// # Control time in tests
///
/// Enable `macros` and `test-util` in an Arty dev-dependency. Add a second
/// [`ClockControl`](crate::time::ClockControl) parameter to
/// [`arty::test`](crate::test) to control the runtime clocks. Time starts at
/// the UNIX epoch and does not advance automatically.
///
/// A delay starts its timer when first polled. Poll it before you advance time:
///
/// ```test_harness
/// use std::future::{Future, poll_fn};
/// use std::pin::pin;
/// use std::task::Poll;
/// use std::time::Duration;
///
/// # #[cfg(all(feature = "macros", feature = "rt", feature = "test-util"))]
/// #[arty::test]
/// async fn controlled_delay(
///     cx: arty::task::Context,
///     control: arty::time::ClockControl,
/// ) {
///     let watch = cx.clock().stopwatch();
///     let mut delay = pin!(cx.clock().delay(Duration::from_secs(30)));
///     poll_fn(|context| {
///         assert!(delay.as_mut().poll(context).is_pending());
///         Poll::Ready(())
///     })
///     .await;
///     control.advance(Duration::from_secs(30));
///     assert_eq!(watch.elapsed(), Duration::from_secs(30));
///     delay.await;
/// }
/// ```
///
/// `auto_advance_timers(true)` advances time when each timer is registered.
/// It does not guarantee which overlapping timer finishes first. Advance time
/// explicitly when the order matters. `auto_advance(duration)` advances time
/// when code reads the clock.
///
/// `ClockControl` works with multiple workers. For a custom runtime builder,
/// create the control in a synchronous test. Pass a clone to
/// [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock).
///
/// # Without an Arty runtime
///
/// `Clock` does not start a timer driver by itself. The `tick::SimpleClock`
/// type reads time without a driver. Delays on [`Clock`](crate::time::Clock)
/// need a driver or controlled time. See [`tick`] for clocks that run outside
/// Arty.
pub mod time {
    #[doc(inline)]
    pub use tick::Clock;
    #[cfg(any(test, feature = "test-util"))]
    #[doc(inline)]
    pub use tick::ClockControl;
}
