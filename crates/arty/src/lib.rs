// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! A runtime for asynchronous tasks with stable thread placement.
//!
//! Use Arty when tasks need to retain thread-local state across asynchronous waits.
//! Each task runs on one worker thread until it completes or is cancelled, so its
//! future can hold non-[`Send`] values such as [`Rc`](std::rc::Rc).
//!
//! Arty provides task scheduling, blocking-task pools, clocks, and telemetry.
//! It does not provide asynchronous I/O drivers or built-in memory pools, and it
//! does not move started tasks between workers to balance their load. Libraries
//! that require another runtime's I/O drivers need that runtime's integration.
//!
//! # Quickstart
//!
//! Enable `macros` to use `#[arty::main]`; it also enables `rt` and `time`.
//! These runtime APIs are not yet published. The following dependency selects a
//! revision that provides them:
//!
//! ```toml
//! [dependencies]
//! arty = { git = "https://github.com/microsoft/oxidizer", rev = "27c6370ea051ece01c519b9a3174413d41b14d64", features = ["macros"] }
//! ```
//!
//! ```rust
//! # #[cfg(feature = "macros")]
//! use arty::runtime::Builtins;
//!
//! # #[cfg(feature = "macros")]
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//!     let answer = cx.scheduler().spawn(async |_| 6 * 7).await?;
//! #   assert_eq!(answer, 42);
//!     println!("{answer}");
//!     Ok(())
//! }
//! # #[cfg(not(feature = "macros"))] fn main() {}
//! ```
//!
//! This prints `42`. The attribute starts a runtime, runs the asynchronous body
//! on a worker, and shuts down when the body returns. `cx` provides that worker's
//! scheduler and clock. Awaiting the child task receives its result without
//! blocking the worker; `?` propagates a task panic or shutdown cancellation.
//!
//! Await any child work that must finish before returning from the entry point.
//! Shutdown cancels pending asynchronous tasks rather than draining them.
//! Use `runtime::Runtime` directly to integrate with synchronous code, borrow
//! caller-owned data, or control when the runtime stops.
//!
//! # Task placement
//!
//! A task's `Builtins::scheduler()` creates child tasks on the same worker.
//! `Runtime::task_scheduler()` distributes submissions across workers.
//! `Builtins::local_scheduler()` also accepts non-`Send` captures and results
//! when called on the associated worker.
//!
//! Cloning a scheduler or `Builtins` preserves its worker association.
//! `TaskScheduler::spawn_anywhere()` can distribute new work and explicitly
//! relocate its payload's capabilities. It does not migrate an existing task
//! or relocate a task's returned value.
//!
//! Keep synchronous blocking calls off asynchronous workers: use
//! `TaskScheduler::spawn_blocking()` so other tasks and timers can make progress.
//!
//! # Documentation
//!
//! The `documentation` module contains guides to scheduling, thread awareness,
//! shutdown, configuration, time, and telemetry. To include all the guides when
//! building documentation from a source checkout, enable all features:
//!
//! ```text
//! cargo doc -p arty --all-features --no-deps --open
//! ```
//!
//! Application dependencies need only the features they use. Enabling all
//! features for documentation does not require using `test-util` in production.
//! Hosted documentation describes published releases, which may not yet contain
//! these runtime APIs.
//!
//! # Features
//!
//! No features are enabled by default.
//!
//! - **`rt`** - Enables `arty::runtime` and `arty::task`, and implies `time`.
//! - **`macros`** - Enables `#[arty::main]` and `#[arty::test]` and implies `rt`.
//! - **`time`** - Exposes time primitives through `arty::time`.
//! - **`test-util`** - Enables testing utilities, including `arty::time::ClockControl`
//!   when `time` is enabled. Enable it in dev-dependencies, not production dependencies.

use arty_io_core as _;
/// Runs an asynchronous entry point on an Arty runtime.
///
/// Apply this attribute to an `async fn` taking one owned
/// [`Builtins`](crate::runtime::Builtins) argument. It creates a synchronous,
/// zero-argument entry point with the same visibility and return type. The
/// asynchronous body runs on an Arty worker, not on the calling thread.
/// Give the argument an identifier such as `cx` or `_cx`, rather than a wildcard
/// or destructuring pattern.
///
/// The runtime stops when the body returns. Await any child tasks that must
/// finish before returning; pending asynchronous work is cancelled at shutdown.
///
/// Enable the `macros` feature to use this attribute.
///
/// # Configuration
///
/// With no options, the attribute uses [`Runtime::new`](crate::runtime::Runtime::new).
/// The following options change construction:
///
/// - `workers = N` uses at most `N` asynchronous workers. `N` must be an
///   integer literal, optionally suffixed with `usize`, that fits the target's
///   `usize`. Fewer available processors means fewer workers, not an error.
///   Zero is rejected during runtime construction.
/// - `builder = expression` supplies a [`RuntimeBuilder`](crate::runtime::RuntimeBuilder).
///   The expression runs once on the calling thread, before workers start.
///   Use it for computed settings, clocks, telemetry, or blocking pools.
/// - `runtime_path = ::renamed_arty::runtime` selects the runtime module when
///   the dependency is renamed or re-exported.
///
/// `workers` and `builder` cannot be combined. A worker limit does not limit
/// blocking-pool threads. Without a limit, the runtime uses
/// [`ProcessorCount::auto`](crate::runtime::ProcessorCount::auto).
///
/// A builder expression cannot refer to the injected argument or variables
/// declared inside the asynchronous body. It must produce a builder; call a
/// factory explicitly, as in `builder = app_builder()`. Use
/// `builder = try_builder()?` when the entry point's return type permits `?`.
/// The attribute does not insert `?` for you. `runtime_path` changes generated
/// references, not paths written inside the builder expression.
///
/// # Panics
///
/// Panics if runtime construction fails or shutdown cancels the root task.
/// If the asynchronous body panics, its original panic payload is resumed on
/// the calling thread. Errors returned by the body remain ordinary return values.
///
/// Use [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) and
/// [`Runtime::run`](crate::runtime::Runtime::run) directly to handle construction
/// and task errors without the attribute converting them to panics. Worker
/// creation and initialization can still panic.
///
/// # Examples
///
/// Wait briefly and print a message:
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::main]
/// async fn main(cx: arty::runtime::Builtins) {
///     cx.clock().delay(std::time::Duration::from_millis(1)).await;
///     println!("Hello from Arty!");
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
///
/// Configure worker and blocking-pool limits with a builder:
///
/// ```
/// # #[cfg(feature = "macros")]
/// fn app_builder() -> arty::runtime::RuntimeBuilder {
///     use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};
///
///     Runtime::builder()
///         .processor_count(ProcessorCount::at_most(4))
///         .blocking_pool_policy(BlockingPoolPolicy::shared(8))
/// }
///
/// # #[cfg(feature = "macros")]
/// #[arty::main(builder = app_builder())]
/// async fn main(cx: arty::runtime::Builtins) -> Result<(), arty::task::JoinError> {
///     let answer = cx.scheduler().spawn(async |_| 42).await?;
///     assert_eq!(answer, 42);
///     Ok(())
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
#[cfg(feature = "macros")]
#[doc(inline)]
pub use arty_macros::main;
/// Runs an asynchronous test on an Arty runtime.
///
/// Apply this attribute to an `async fn` taking one owned
/// [`Builtins`](crate::runtime::Builtins) argument. It creates a synchronous,
/// zero-argument test and runs the body on a worker. Each invocation starts its
/// own runtime and shuts it down when the body returns.
/// Give the argument an identifier; use `_cx` when its value is not needed.
///
/// The body's return value, visibility, and test attributes such as
/// `#[should_panic]` and `#[ignore]` are preserved. An ignored test does not
/// construct a runtime unless the test harness is told to run it.
///
/// Enable the `macros` feature to use this attribute. The [`main`] attribute's
/// `workers`, `builder`, and `runtime_path` options also apply. Omitting
/// `workers` uses the automatic processor policy, not a single-worker runtime.
///
/// # Simulated time
///
/// Enable `test-util` in an Arty dev-dependency and add a second owned
/// `arty::time::ClockControl` argument to use controlled time. It starts at the
/// UNIX epoch with automatic advancement disabled. The control and the runtime's
/// clocks share the same time domain; cloned controls affect those same clocks.
/// Use distinct identifiers for the arguments. Type aliases and associated
/// types are supported.
///
/// Use `control.advance(duration)` to advance time manually. A delay must be
/// polled before advancing time, because its first poll registers the timer.
/// `auto_advance_timers(true)` is convenient for sequential delays, but advances
/// eagerly on timer registration or time reads; it does not wait for the runtime
/// to become idle or preserve concurrent timers' deadline order.
///
/// The clock-control argument can be combined with `workers`, but not `builder`.
/// To configure both a builder and controlled time, use a synchronous test that
/// creates a control and calls `builder.clock(control.clone()).build()`.
/// Enabling `test-util` alone does not change a one-argument test's clock.
///
/// # Panics
///
/// Panics if runtime construction fails or shutdown cancels the test body.
/// A panic in the body is resumed with its original payload, so
/// `#[should_panic(expected = "...")]` can match the original message.
///
/// # Examples
///
/// Await a task and check its result:
///
/// ```test_harness
/// # #[cfg(feature = "macros")]
/// #[arty::test]
/// async fn answer(cx: arty::runtime::Builtins) -> Result<(), arty::task::JoinError> {
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
///     Ok(())
/// }
/// ```
///
/// Test a sequential delay without waiting for real time:
///
/// ```test_harness
/// # #[cfg(all(feature = "macros", feature = "test-util"))]
/// #[arty::test]
/// async fn simulated_delay(cx: arty::runtime::Builtins, control: arty::time::ClockControl) {
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

#[cfg(all(any(doc, test), feature = "rt", feature = "macros", feature = "time", feature = "test-util"))]
pub mod documentation;

#[cfg(any(test, feature = "rt"))]
pub mod runtime;
#[cfg(any(test, feature = "rt"))]
pub mod task;

/// Worker coordinates and traits for relocating thread-aware values.
///
/// [`Thread`](crate::core::Thread) identifies an [`Owner`](crate::core::Owner),
/// an OS thread, and its [`NumaNode`](crate::core::NumaNode).
/// Use it to describe where a value's capabilities belong. It is a coordinate,
/// not a thread handle: retaining it does not keep a thread or runtime alive.
///
/// Implement [`ThreadAware`](crate::core::ThreadAware) when a transferable value needs to update its
/// worker-bound capabilities after explicit relocation. Neither cloning a
/// coordinate nor relocating a value changes the OS thread executing your code.
pub mod core {
    #[doc(inline)]
    pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
}

/// Clocks, timers, and timeouts.
///
/// Enable `time` to use these [`tick`] types without starting an Arty runtime.
/// Inside an Arty task, use `Builtins::clock()` for a clock whose timers the
/// worker drives. Outside the runtime, a [`Clock`](crate::time::Clock) still needs
/// a timer driver for delays; [`SimpleClock`](crate::time::SimpleClock) provides
/// time queries without timers.
///
/// Enable `test-util` in dev-dependencies to use `ClockControl` for simulated time.
///
/// # Examples
///
/// With `macros` enabled, await a delay on the task's clock:
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::main]
/// async fn main(cx: arty::runtime::Builtins) {
///     let duration = std::time::Duration::from_millis(1);
///     let watch = cx.clock().stopwatch();
///     cx.clock().delay(duration).await;
///     assert!(watch.elapsed() >= duration);
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
#[cfg(any(test, feature = "time"))]
pub mod time {
    #[cfg(any(test, feature = "test-util"))]
    #[doc(inline)]
    pub use tick::ClockControl;
    #[doc(inline)]
    pub use tick::{Clock, Delay, FutureExt, PeriodicTimer, SimpleClock, Stopwatch, Timeout};
}

#[cfg(test)]
testing_aids::init_tracing!();
