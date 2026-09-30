// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! A runtime for worker-local asynchronous tasks.
//!
//! Arty makes worker-local execution and explicit relocation of runtime capabilities
//! the core programming model. It is intended for applications that organize work
//! around state owned by individual workers, rather than treating locality as an
//! addition to a general-purpose scheduler.
//!
//! Unlike [Tokio's multithreaded scheduler](https://docs.rs/tokio/latest/tokio/runtime/#multi-thread-scheduler),
//! Arty does not move started tasks between workers to balance their load. Each task
//! stays on its worker, where it can create and retain non-[`Send`] state.
//! Tokio also supports [local tasks](https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html);
//! the distinction is Arty's worker-local model, not exclusive support for non-`Send` futures.
//! The trade-off is that a busy worker's tasks are not redistributed. Arty provides
//! scheduling, blocking tasks, clocks, and telemetry, but no asynchronous I/O drivers
//! or built-in memory pools. It is not a drop-in replacement for Tokio.
//!
//! **Thread awareness** means that runtime capabilities have an explicit worker
//! association. Each task receives an owned `Builtins` value containing its worker's
//! scheduler and clock. Cloning preserves that association; explicit relocation can
//! rebind capabilities to another worker in the same runtime. Relocation does not
//! migrate a running task or automatically relocate ordinary task results.
//!
//! # Quickstart
//!
//! Enable `macros` to use the runtime entry points; it also enables `rt` and
//! `time`. To try the runtime from this repository, use a Git revision that
//! contains these APIs:
//!
//! ```toml
//! [dependencies]
//! arty = { git = "https://github.com/microsoft/oxidizer", rev = "65f7f337b14b59259ad500484439a77f1f7f4f23", features = ["macros"] }
//! ```
//!
//! ```rust
//! # #[cfg(feature = "macros")]
//! use arty::runtime::Builtins;
//!
//! # #[cfg(feature = "macros")]
//! #[arty::main]
//! async fn main(cx: Builtins) {
//!     let answer = cx
//!         .scheduler()
//!         .spawn(async |_| 6 * 7)
//!         .await
//!         .expect("the child task completes before the entry point returns");
//! #   assert_eq!(answer, 42);
//!     println!("{answer}");
//! }
//! # #[cfg(not(feature = "macros"))] fn main() {}
//! ```
//!
//! This prints `42`. The attribute creates the runtime and runs the entry point
//! on a worker, passing owned `Builtins`. Its scheduler creates the child task
//! on that worker; `.await` observes a `Result` without blocking the worker.
//! Task panics and shutdown cancellation are reported as `arty::task::JoinError`.
//! The runtime shuts down when the entry point finishes, so await any required
//! child work before returning. Use `arty::runtime::Runtime` directly when
//! integrating with synchronous code or controlling ownership and shutdown.
//!
//! # Documentation
//!
//! `arty::documentation` contains longer guides to scheduling,
//! thread awareness, lifecycle, configuration, time, and telemetry. It is included
//! in documentation and test builds only when all Arty features are enabled.
//! The dependency revision above selects the runtime API and predates these
//! guides. From a source checkout containing the documentation module, run:
//!
//! ```text
//! cargo doc -p arty --all-features --no-deps --open
//! ```
//!
//! Select `documentation` in the generated API reference's module index.
//! Hosted docs describe published releases and may not yet include guides from
//! unreleased source. Application dependencies need only the features they use;
//! building all-feature documentation does not require a production `test-util`
//! dependency.
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

#[cfg(feature = "macros")]
mod macros;
#[cfg(feature = "macros")]
pub use macros::{main, test};

#[cfg(all(any(doc, test), feature = "rt", feature = "macros", feature = "time", feature = "test-util"))]
pub mod documentation;

#[cfg(any(test, feature = "rt"))]
pub mod runtime;
#[cfg(any(test, feature = "rt"))]
pub mod task;

/// Coordinates and traits for thread-aware values.
///
/// [`Thread`](crate::core::Thread) describes a runtime owner, an OS thread, and its NUMA locality;
/// it is not a thread handle and does not keep the thread alive.
/// [`ThreadAware`](crate::core::ThreadAware) lets a value adapt its capabilities after explicit relocation.
/// Neither cloning a coordinate nor notifying a value moves the executing OS thread.
pub mod core {
    #[doc(inline)]
    pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
}

/// Clocks, timers, and timeouts.
///
/// These are re-exported from [`tick`] and are available with `time`, independently
/// of the runtime. Inside an Arty task, obtain the worker-driven clock through
/// `Builtins::clock()`. A clock used outside Arty still needs an appropriate timer
/// driver; enabling `time` alone does not start one.
///
/// Enable `test-util` in dev-dependencies to control time with `ClockControl`.
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
