// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Single-threaded, thread-aware application runtime.
//!
//! Arty is being developed as a small runtime. Stable contracts for integrating external I/O
//! drivers live in [`arty_io_core`].
//!
//! The runtime surface is still taking shape, so the crate currently re-exports the
//! thread-awareness types that integrators build on:
//!
//! ```
//! use arty::core::{NumaNode, Owner, Thread, ThreadAware};
//! ```
//!
//! # Features
//!
//! - **`time`** - Exposes time primitives through `arty::time`.
//! - **`test-util`** - Enables test-only runtime utilities. With `time`, this includes
//!   `arty::time::ClockControl`.
//!
//! # Project policies
//!
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/DESIGN.md)
//! - [I/O](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md)
//! - [Panics](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md)
//! - [Stabilization](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/STABILIZATION.md)

use arty_io_core as _;

pub mod core {
    //! Foundational runtime and thread-awareness types.
    //!
    //! Re-exports [`thread_aware_core`] types used to identify runtime owners,
    //! threads, and NUMA nodes when integrating with arty.

    pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
}

#[cfg(any(test, feature = "time"))]
pub mod time {
    //! Time primitives for the runtime.
    //!
    //! Re-exports [`tick`] clock, delay, timeout, and stopwatch types. With
    //! the `test-util` feature, also re-exports [`tick::ClockControl`] for
    //! deterministic control of time in tests.

    #[cfg(any(test, feature = "test-util"))]
    pub use tick::ClockControl;
    pub use tick::{Clock, Delay, FutureExt, PeriodicTimer, SimpleClock, Stopwatch, Timeout};
}
