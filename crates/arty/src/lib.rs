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
