// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(docsrs, feature = "rt", feature = "macros", feature = "test-util")),
    expect(rustdoc::broken_intra_doc_links, reason = "feature-gated items and docs.rs-only guides")
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Single-threaded, thread-aware application runtime.
//!
//! An Arty runtime defaults to one worker and can be configured with several,
//! but each async task stays on one worker throughout its life. This lets it use
//! thread-local and non-[`Send`] state across awaits. Thread-aware values can be
//! explicitly relocated when starting new work on another worker.
//!
//! There is no process-global runtime: each instance owns its workers and
//! services. Workers use thread-local bookkeeping rather than a global
//! runtime singleton.
//!
//! Arty provides task scheduling, clocks, external I/O driver hosting, and
//! pools for offloading blocking callbacks. It does not provide an async I/O
//! implementation or move running tasks between workers.
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
//! Await child work before returning; shutdown cancels pending async tasks.
//!
//! # Overview
//!
//! - [`main`] and [`test`] manage a runtime for an
//!   async entry point or test; [`Runtime`](crate::runtime::Runtime) gives you
//!   direct control over its lifetime and settings.
//! - [`Context`](crate::task::Context) gives each task its scheduler, clock,
//!   and worker. [`RuntimeScheduler`](crate::task::RuntimeScheduler) lets the
//!   runtime place new work.
//! - [`Io`](crate::io::Io) registers external `arty_io_core` contexts and lazily
//!   initializes a driver on each worker when that worker first requests the
//!   context.
//! - [`Scheduler`](crate::task::Scheduler) keeps child work on its worker.
//! - [`Clock`](crate::time::Clock) provides timers and timeouts;
//!   [`ClockControl`](crate::time::ClockControl) controls time in tests.
//! - [`Thread`](core::Thread) describes a worker, and
//!   [`ThreadAware`](core::ThreadAware) values can relocate between workers.
//!
//! # Why Arty?
//!
//! Unlike Tokio's multi-thread runtime, Arty keeps each task on one worker.
//! This supports thread-local and non-`Send` state, keeps worker-local data
//! nearby, and can reduce contention for that data. Shared data may still contend.
//! Choose Tokio for automatic task distribution or its async I/O ecosystem;
//! Arty does not provide Tokio's I/O implementations.
//!
//! # Blocking work
//!
//! Join handles are futures: await them. Arty deliberately does not expose a
//! synchronous join operation because async workers must stay free to poll
//! tasks and advance timers. Blocking a worker stalls every task assigned to
//! it. If the blocked code waits for work on that worker, or blocking callbacks
//! wait on each other in an exhausted pool, the stall can become a deadlock.
//!
//! This deadlocks: the worker waits synchronously for a child that only the
//! same worker can poll.
//!
//! ```no_run
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! #[arty::main]
//! async fn main(cx: arty::task::Context) {
//!     let child = cx.scheduler().spawn(async |_| 42);
//!     let _ = futures::executor::block_on(child);
//! }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on) is
//! the narrow blocking entry point for running async work from synchronous
//! code. It rejects calls from async workers. For synchronous I/O or library
//! calls, use
//! [`Scheduler::spawn_blocking`](crate::task::Scheduler::spawn_blocking) or
//! [`RuntimeScheduler::spawn_blocking`](crate::task::RuntimeScheduler::spawn_blocking)
//! and await the join. The callback runs in a blocking pool instead of on an
//! async worker.
//!
//! # Detailed documentation
//!
//! The [guides](crate::documentation) explain Arty's capabilities in more detail:
//!
//! - [Scheduling](crate::documentation::scheduling) explains where tasks run,
//!   local non-`Send` work, and blocking pools.
//! - [Configuration](crate::documentation::configuration) explains worker counts,
//!   blocking-pool policies, and clocks.
//! - [Shutdown](crate::documentation::shutdown) explains runtime ownership,
//!   task cancellation, and what stopping the workers waits for.
//! - [Thread awareness](crate::documentation::thread_awareness) explains
//!   stable task placement and explicit relocation of values.
//! - [Time](crate::documentation::time) explains system timers, timeouts, and
//!   externally controlled time in tests.
//!
//! The `documentation` module is included only for docs.rs builds and
//! doc-test collection with all documentation features, including `test-util`;
//! it is not part of the public API available to applications.
//!
//! # Features
//!
//! The default feature set enables `rt`, `macros`, and `monitor`.
//!
//! - **`rt`** - Enables the runtime, task, and I/O APIs.
//! - **`macros`** - Enables [`main`] and [`test`], and implies `rt`.
//! - **`monitor`** - Enables the Seismograph monitor dependency. It forwards
//!   `performables` telemetry support when `performables` is enabled. It does
//!   not start monitoring or emit new telemetry.
//! - **`test-util`** - Enables testing utilities, including [`ClockControl`](crate::time::ClockControl).
//!   Enable it in dev-dependencies, not production dependencies.

use arty_io_core as _;
/// Runs an async entry point on an Arty runtime.
///
/// Apply it to an `async fn` taking one owned
/// [`Context`](crate::task::Context) argument. It generates a synchronous
/// entry point with no arguments and the same visibility and return type.
/// The async body runs on an Arty worker, not on the calling thread. Name the
/// argument `cx` or `_cx`, for example; `_` and destructuring patterns are not supported.
///
/// The runtime stops when the body returns. Await child tasks you need first;
/// shutdown cancels pending async work.
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
/// An application error returned by the body remains its return value; it is
/// not converted to a panic. Use [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build),
/// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on), and
/// [`Runtime::stop`](crate::runtime::Runtime::stop) directly to handle returned
/// runtime errors. Worker-thread creation and initialization can still panic.
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
/// automatically; cloned controls share that time. Use different names for
/// the two arguments. Type aliases and associated types are supported.
///
/// Use `control.advance(duration)` to move time forward. Poll a delay before
/// advancing: it starts its timer on the first poll. For sequential delays,
/// `auto_advance_timers(true)` advances time eagerly; it does not wait for an
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
/// with its original payload, so
/// `#[should_panic(expected = "...")]` can match the original message.
/// If a runtime failure cancels the test body, the panic message includes that
/// failure.
///
/// An application error returned by the body remains its return value. Use
/// explicit runtime construction, [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on),
/// and [`Runtime::stop`](crate::runtime::Runtime::stop) when runtime errors
/// should be handled without the attribute converting them to panics.
///
/// # Examples
///
/// Await a task and check its result:
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

#[cfg(all(
    doc,
    any(docsrs, doctest),
    feature = "rt",
    feature = "macros",
    feature = "test-util"
))]
pub mod documentation;

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
/// where a value belongs; it does not own that thread or keep its runtime running.
///
/// Implement [`ThreadAware`](crate::core::ThreadAware) when a value needs to
/// update worker-bound state after relocation. Relocation does not move a
/// running task or change the OS thread executing your code.
pub mod core {
    #[doc(inline)]
    pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
}

/// Runtime clocks and controlled test time.
///
/// [`Clock`](crate::time::Clock) is always available, even without an Arty
/// runtime. A task's `Context::clock()` has timers driven by its worker.
/// Use the underlying [`tick`] API for timer future types and extension traits.
///
/// Enable `test-util` in dev-dependencies to use `ClockControl` for simulated time.
///
/// # Examples
///
/// With `rt` and `macros` enabled, await a delay on the task's clock:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
/// # async fn main(cx: arty::task::Context) {
/// let duration = std::time::Duration::from_millis(1);
/// let watch = cx.clock().stopwatch();
/// cx.clock().delay(duration).await;
/// assert!(watch.elapsed() >= duration);
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
pub mod time {
    #[doc(inline)]
    pub use tick::Clock;
    #[cfg(any(test, feature = "test-util"))]
    #[doc(inline)]
    pub use tick::ClockControl;
}
