// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(feature = "rt", feature = "macros", feature = "time", feature = "test-util")),
    expect(
        rustdoc::broken_intra_doc_links,
        reason = "feature-gated items are unavailable in partial documentation builds"
    )
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Cooperative single-threaded runtime for thread-aware applications.
//!
//! Each task stays on one worker, which supports thread-local and non-[`Send`]
//! state across awaits. Shared handles can use explicit [`ThreadAware`](core::ThreadAware)
//! relocation to expose one logical instance across workers while keeping
//! worker-local state separate.
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
//! #[arty::main]
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
//! The `?` returns a [`JoinError`](crate::task::JoinError) if the child fails.
//! Await child work before returning. Shutdown cancels pending async tasks.
//!
//! # Execution model
//!
//! Each Arty worker runs a cooperative single-threaded executor. A task returns
//! control when an awaited operation is pending, and it stays on one worker
//! throughout its life. Arty does not move running tasks between workers.
//!
//! A task factory creates the task future on its destination worker. A payload
//! is a separate value passed to that factory.
//!
//! [`ThreadAware`](core::ThreadAware) gives a type a callback for explicit
//! relocation. For [`Scheduler::spawn_anywhere`](crate::task::Scheduler::spawn_anywhere),
//! Arty selects the destination. For
//! [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at), Arty validates the
//! requested destination. If the task factory starts, Arty relocates the payload
//! before the factory uses it. Rejected work and work cancelled before the
//! factory starts can drop the payload without relocation.
//!
//! Shared handles can use this callback to expose one logical instance across
//! workers while each clone uses worker-local state. `ThreadAware` does not
//! provide shared ownership or local storage by itself.
//!
//! This example passes `cx.clone()` as the thread-aware payload. The closure is
//! the task factory. Arty relocates the payload before the factory uses it:
//!
//! ```
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! # #[arty::main]
//! # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
//! cx.scheduler()
//!     .spawn_anywhere(cx.clone(), |moved| async move {
//!         assert_eq!(moved.thread().id(), std::thread::current().id());
//!     })
//!     .await?;
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! The runtime can select the original worker. The invariant is that the
//! relocated context reports the worker that runs the new task.
//!
//! There is no process-global runtime. Each instance owns its workers and
//! services. Workers use thread-local bookkeeping rather than a global runtime
//! singleton.
//!
//! Arty provides task scheduling, clocks, blocking pools, and a host for
//! external I/O drivers. I/O libraries can integrate through `arty_io_core`.
//! Companion I/O crates ship alongside Arty.
//!
//! # API overview
//!
//! - [`runtime`] covers construction, configuration, ownership, and shutdown.
//! - [`task`] covers task placement, worker-local futures, blocking callbacks,
//!   and result handles.
//! - [`time`] covers clocks, delays, timeouts, and controlled test time.
//! - [`io`] covers external I/O contexts and worker-local drivers.
//! - [`core`] contains worker coordinates and the thread-awareness contract.
//! - [`main`] and [`test`] manage a runtime for an async entry point or test.
//!
//! # Features
//!
//! The default feature set enables `rt`, `macros`, and `monitor`.
//!
//! - `rt`: Enables the runtime, task, and I/O APIs, and implies `time`.
//! - `macros`: Enables [`main`] and [`test`], and implies `rt`.
//! - `monitor`: Enables the Seismograph monitor dependency. It forwards
//!   `performables` telemetry support when `performables` is enabled. It does
//!   not start monitoring.
//! - `time`: Enables clocks, timers, and timeouts.
//! - `test-util`: Enables testing utilities, including
//!   [`ClockControl`](crate::time::ClockControl) with `time`.
//!   Enable it in dev-dependencies, not production dependencies.

use arty_io_core as _;
/// Runs an async entry point on an Arty runtime.
///
/// Apply it to an `async fn` taking one owned
/// [`Context`](crate::task::Context) argument. It generates a synchronous
/// entry point with no arguments and the same visibility and return type.
/// The async body runs on an Arty worker, not on the calling thread. For
/// example, name the argument `cx` or `_cx`. The argument must be an identifier.
/// The attribute does not support `_` or destructuring patterns.
///
/// The runtime stops when the body returns. Await required child tasks first.
/// Shutdown cancels pending async work.
///
/// Enable the `macros` feature to use this attribute.
///
/// # Configuration
///
/// Without options, the attribute uses
/// [`Runtime::new`](crate::runtime::Runtime::new). The optional configuration is:
///
/// - `workers = N` starts exactly `N` async workers. Use an integer literal
///   that fits `usize` (an explicit `usize` suffix is fine). If fewer processors
///   are available, multiple worker threads share each processor. Zero fails at
///   construction.
/// - `workers = all` starts one async worker per available processor.
/// - `monitor = enabled` or `monitor = disabled` selects the runtime monitoring
///   policy. The default is `enabled`.
/// - `panic = shutdown` or `panic = ignore` selects the runtime panic policy.
///   The default is `shutdown`.
///
/// The worker setting does not limit blocking-pool threads. Without a setting, the
/// runtime uses [`WorkersPolicy::default`](crate::runtime::WorkersPolicy::default).
/// The default is one worker, for both applications and tests.
/// Each worker uses an operating-system thread and its configured stack.
/// Large counts can exceed operating-system thread or memory limits.
///
/// Use explicit [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) construction
/// instead of the attribute when clocks, blocking pools, stack size,
/// or an at-most worker policy must be configured.
///
/// # Panics
///
/// Runtime construction errors, root-task cancellation, and shutdown errors
/// become panics. The runtime is stopped before returning the body's value or
/// resuming its original panic on the calling thread. A root-task failure takes
/// precedence if shutdown also fails. If a runtime failure cancels the root
/// task, the panic message includes that failure.
///
/// An application error returned by the body remains its return value. The
/// attribute does not convert it to a panic. Use
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build),
/// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on), and
/// [`Runtime::stop`](crate::runtime::Runtime::stop) to handle runtime errors
/// directly. Worker-thread creation and initialization can still panic.
///
/// # Examples
///
/// Wait briefly and print a message:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
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
/// Keep the runtime available after a contained panic:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main(panic = ignore)]
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
/// #[arty::main(workers = 4)]
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
/// The return value, visibility, and attributes such as `#[should_panic]`
/// and `#[ignore]` are preserved. An ignored test starts no runtime unless
/// the test harness runs it.
///
/// Enable `macros` to use this attribute. The [`main`] attribute's `workers`,
/// `monitor`, and `panic` options also apply. By default, a test uses one worker,
/// selects the disabled monitoring policy, and uses the shutdown panic policy.
/// This keeps future monitor activation out of ordinary tests. A test that
/// exercises monitoring must opt in with `monitor = enabled`.
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
/// advancing: it starts its timer on the first poll. For sequential delays,
/// `auto_advance_timers(true)` advances time eagerly. It does not wait for an
/// idle runtime or guarantee the order of concurrent timers.
///
/// The control argument works with `workers`. To use a custom runtime builder
/// with controlled time, create the control in a synchronous test and call
/// `builder.clock(control.clone()).build()`. Enabling `test-util` alone does
/// not change a one-argument test's clock.
///
/// # Panics
///
/// Runtime construction errors, test-body cancellation, and shutdown errors
/// become panics. The runtime is stopped before reporting the test's outcome.
/// A panic in the body takes precedence over a shutdown error and is resumed
/// with its original payload. Therefore,
/// `#[should_panic(expected = "...")]` can match the original message.
/// If a runtime failure cancels the test body, the panic message includes that
/// failure.
///
/// An application error returned by the body remains its return value. Use
/// explicit runtime construction when the caller must handle runtime errors
/// without converting them to panics. Use
/// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on) and
/// [`Runtime::stop`](crate::runtime::Runtime::stop) to run and stop the runtime.
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
/// ```test_harness
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::test(monitor = enabled)]
/// async fn monitored(_cx: arty::task::Context) {}
/// ```
///
/// Keep the test runtime available after a contained child-task panic:
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

/// Worker coordinates and thread-aware relocation.
///
/// [`Thread`](crate::core::Thread) records a runtime [`Owner`](crate::core::Owner),
/// an OS thread, and its [`NumaNode`](crate::core::NumaNode). It describes
/// where a value belongs. It does not own that thread or keep its runtime
/// running.
///
/// Implement [`ThreadAware`](crate::core::ThreadAware) when a value needs to
/// update worker-bound state after relocation. Relocation does not move a
/// running task or change the OS thread executing your code.
///
/// # Example
///
/// Read the coordinate of the worker that runs a task. This example requires
/// `macros`, which enables `rt`:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
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
/// Arty re-exports [`Clock`](crate::time::Clock) and, with `test-util`,
/// [`ClockControl`](crate::time::ClockControl). Use [`tick`] directly for delay
/// and timeout futures, extension traits, stopwatches, and periodic timers.
/// Add `tick` as a direct dependency before you use these APIs.
/// The `time` feature works without an Arty runtime. The `rt` feature also
/// enables time and drives system timers for its workers.
///
/// # Use the task clock
///
/// Each task gets a clock through
/// [`Context::clock`](crate::task::Context::clock). Use a stopwatch for elapsed
/// time. Use `system_time()` for a wall-clock timestamp, which can change
/// independently of elapsed time. This example requires `macros`, which enables
/// `rt` and `time`.
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
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
/// It does not guarantee the order of concurrent timers. Advance time
/// explicitly when the order matters. `auto_advance(duration)` advances time
/// when code reads the clock.
///
/// `ClockControl` works with multiple workers. For a custom runtime builder,
/// create the control in a synchronous test. Pass a clone to
/// [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock).
///
/// # Without an Arty runtime
///
/// Enabling `time` alone does not start a timer driver. The
/// `tick::SimpleClock` type reads time without a driver. Delays on
/// [`Clock`](crate::time::Clock) need a driver or controlled time. See [`tick`]
/// for clocks that run outside Arty.
#[cfg(any(test, feature = "time"))]
pub mod time {
    #[doc(inline)]
    pub use tick::Clock;
    #[cfg(any(test, feature = "test-util"))]
    #[doc(inline)]
    pub use tick::ClockControl;
}
