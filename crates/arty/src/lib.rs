// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(docsrs, feature = "rt", feature = "macros", feature = "time", feature = "test-util")),
    expect(rustdoc::broken_intra_doc_links, reason = "feature-gated items and docs.rs-only guides")
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Single-threaded, thread-aware application runtime.
//!
//! An Arty runtime can have several workers, but each async task stays on one
//! worker throughout its life. This lets it use thread-local and non-[`Send`]
//! state across awaits. Thread-aware values can be explicitly relocated when
//! starting new work on another worker.
//!
//! There is no process-global runtime: each instance owns its workers and
//! services. Workers use thread-local bookkeeping rather than a global
//! runtime singleton.
//!
//! Arty provides task scheduling, blocking-task pools, clocks, and telemetry.
//! It does not provide async I/O drivers or move running tasks between workers.
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
//! use arty::task::{Builtins, JoinError};
//!
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), JoinError> {
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
//! - [`Builtins`](crate::task::Builtins) gives each task its scheduler, clock,
//!   and worker. [`RuntimeScheduler`](crate::task::RuntimeScheduler) lets the
//!   runtime place new work.
//! - [`Scheduler`](crate::task::Scheduler) keeps child work on its
//!   worker; [`LocalScheduler`](crate::task::LocalScheduler) lets tasks
//!   share non-`Send` state there.
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
//! Arty does not provide Tokio's I/O or timer drivers.
//!
//! # Detailed documentation
//!
//! The [guides](crate::documentation) explain Arty's capabilities in more detail:
//!
//! - [Scheduling](crate::documentation::scheduling) explains where tasks run,
//!   local non-`Send` work, and blocking pools.
//! - [Configuration](crate::documentation::configuration) explains worker counts,
//!   blocking-pool policies, clocks, and telemetry sinks.
//! - [Shutdown](crate::documentation::shutdown) explains runtime ownership,
//!   task cancellation, and what stopping the workers waits for.
//! - [Thread awareness](crate::documentation::thread_awareness) explains
//!   stable task placement and explicit relocation of values.
//! - [Time](crate::documentation::time) explains worker-driven timers, timeouts,
//!   and controlled time in tests.
//! - [Telemetry](crate::documentation::telemetry) explains runtime events
//!   and links to [`observed`] for further details.
//!
//! The `documentation` module is included only for docs.rs builds and
//! doc-test collection with all documentation features, including `test-util`;
//! it is not part of the public API available to applications.
//!
//! # Features
//!
//! The default feature set enables `rt` and `macros`.
//!
//! - **`rt`** - Enables the runtime and task APIs, and implies `time`.
//! - **`macros`** - Enables [`main`] and [`test`], and implies `rt`.
//! - **`time`** - Enables clocks, timers, and timeouts.
//! - **`test-util`** - Enables testing utilities, including [`ClockControl`](crate::time::ClockControl) with `time`.
//!   Enable it in dev-dependencies, not production dependencies.

use arty_io_core as _;
/// Runs an async entry point on an Arty runtime.
///
/// Apply it to an `async fn` taking one owned
/// [`Builtins`](crate::task::Builtins) argument. It generates a synchronous
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
/// [`Runtime::new`](crate::runtime::Runtime::new). You can instead specify:
///
/// - `workers = N` caps the number of async workers. Use an integer literal
///   that fits `usize` (an explicit `usize` suffix is fine). If fewer processors
///   are available, Arty starts fewer workers; zero fails at construction.
/// - `builder = expression` supplies a
///   [`RuntimeBuilder`](crate::runtime::RuntimeBuilder). It runs once on the
///   calling thread before workers start; use it for clocks, telemetry, pools,
///   or other custom settings.
/// - `runtime_path = ::renamed_arty::runtime` points generated code at a
///   renamed or re-exported Arty runtime module.
///
/// `workers` and `builder` cannot be combined. A worker limit does not limit
/// blocking-pool threads. Without a limit, the runtime uses
/// [`CpuPolicy::auto`](crate::runtime::CpuPolicy::auto).
///
/// The builder expression runs before the async body, so it cannot use the
/// injected argument or variables declared inside the body. It must produce
/// a builder: write `builder = app_builder()` or `builder = try_builder()?`
/// when the entry point's return type allows `?`. The attribute does not
/// insert `?`. `runtime_path` does not change paths you write in the builder.
///
/// # Panics
///
/// Runtime construction errors, root-task cancellation, and shutdown errors
/// become panics. The runtime is stopped before returning the body's value or
/// resuming its original panic on the calling thread. A root-task failure takes
/// precedence if shutdown also fails.
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
/// #[arty::main]
/// async fn main(cx: arty::task::Builtins) {
///     cx.clock().delay(std::time::Duration::from_millis(1)).await;
///     println!("Hello from Arty!");
/// }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
///
/// Configure worker and blocking-pool limits with a builder:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// fn app_builder() -> arty::runtime::RuntimeBuilder {
///     use arty::runtime::{BlockingPoolPolicy, CpuPolicy, Runtime};
///
///     Runtime::builder()
///         .cpu_policy(CpuPolicy::at_most(4))
///         .blocking_pool_policy(BlockingPoolPolicy::shared(8))
/// }
///
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main(builder = app_builder())]
/// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
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
/// [`Builtins`](crate::task::Builtins) argument. It registers a synchronous
/// test that starts its own runtime, runs the body on a worker, and stops the
/// runtime when the body returns.
/// Name the argument `_cx` if it is unused.
///
/// The return value, visibility, and attributes such as `#[should_panic]`
/// and `#[ignore]` are preserved. An ignored test starts no runtime unless
/// the test harness runs it.
///
/// Enable `macros` to use this attribute. The [`main`] attribute's `workers`,
/// `builder`, and `runtime_path` options also apply. By default, a test uses
/// one worker; `workers` or `builder` can change that.
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
/// The control argument works with `workers`, but not `builder`. To use a
/// custom builder with controlled time, create the control in a synchronous
/// test and call `builder.clock(control.clone()).build()`. Enabling
/// `test-util` alone does not change a one-argument test's clock.
///
/// # Panics
///
/// Runtime construction errors, test-body cancellation, and shutdown errors
/// become panics. The runtime is stopped before reporting the test's outcome.
/// A panic in the body takes precedence over a shutdown error and is resumed
/// with its original payload, so
/// `#[should_panic(expected = "...")]` can match the original message.
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
/// async fn answer(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
///     Ok(())
/// }
/// ```
///
/// Test a sequential delay without waiting for real time:
///
/// ```test_harness
/// # #[cfg(all(feature = "macros", feature = "rt", feature = "test-util"))]
/// #[arty::test]
/// async fn simulated_delay(cx: arty::task::Builtins, control: arty::time::ClockControl) {
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
    feature = "time",
    feature = "test-util"
))]
pub mod documentation;

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

/// Clocks, timers, and timeouts.
///
/// Enable `time` to use [`tick`] clocks without starting an Arty runtime.
/// A task's `Builtins::clock()` has timers driven by its worker. Outside the
/// runtime, delays on [`Clock`](crate::time::Clock) need a timer driver;
/// `tick::SimpleClock` can read time without a timer driver when the underlying
/// tick API is needed directly.
///
/// Enable `test-util` in dev-dependencies to use `ClockControl` for simulated time.
///
/// # Examples
///
/// With `rt` and `macros` enabled, await a delay on the task's clock:
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main]
/// async fn main(cx: arty::task::Builtins) {
///     let duration = std::time::Duration::from_millis(1);
///     let watch = cx.clock().stopwatch();
///     cx.clock().delay(duration).await;
///     assert!(watch.elapsed() >= duration);
/// }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[cfg(any(test, feature = "time"))]
pub mod time {
    #[cfg(any(test, feature = "test-util"))]
    #[doc(inline)]
    pub use tick::ClockControl;
    #[doc(inline)]
    pub use tick::{Clock, Delay, FutureExt, PeriodicTimer, Stopwatch, Timeout};
}

#[cfg(test)]
testing_aids::init_tracing!();
