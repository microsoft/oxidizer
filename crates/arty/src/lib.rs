// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Thread-aware, thread-per-core application runtime.
//!
//! Each worker has a single-threaded executor: a task remains on its original worker for its
//! entire lifetime. An `arty::runtime::Runtime` owns worker startup and shutdown. Its
//! `task_scheduler()` distributes work round-robin, while a task's
//! `Builtins::scheduler` preserves worker affinity. Futures are constructed on the destination
//! worker and need not be [`Send`].
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "rt")] {
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new().unwrap();
//! let scheduler = runtime.task_scheduler();
//! let answer = scheduler
//!     .spawn(async |cx| cx.scheduler().spawn(async |_| 42).await)
//!     .wait();
//! assert_eq!(answer, 42);
//! # }
//! # }
//! ```
//!
//! Arty provides scheduling, blocking system tasks, clocks, and structured telemetry. It does
//! not provide asynchronous I/O drivers or memory pools. External I/O integration through
//! [`arty_io_core`] is planned separately.
//!
//! # Features
//!
//! No features are enabled by default.
//!
//! - **`rt`** - Enables `arty::runtime` and `arty::task`, and implies `time`.
//! - **`macros`** - Enables `#[arty::runtime::main]` and `#[arty::runtime::test]` and implies `rt`.
//! - **`time`** - Exposes time primitives through `arty::time`.
//! - **`test-util`** - Enables test-only runtime utilities. With `time`, this includes
//!   `arty::time::ClockControl`. Under Miri, runtime tests use a simulated
//!   six-processor topology instead of native processor discovery and pinning.
//!
//! # Project policies
//!
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/DESIGN.md)
//! - [I/O](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md)
//! - [Panics](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md)
//! - [Stabilization](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/STABILIZATION.md)

use arty_io_core as _;

#[cfg(any(test, feature = "rt"))]
pub mod runtime;
#[cfg(any(test, feature = "rt"))]
pub mod task;

/// Foundational runtime and thread-awareness types.
pub mod core {
    #[doc(inline)]
    pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
}

/// Time primitives for the runtime.
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
