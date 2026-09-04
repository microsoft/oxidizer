// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]
#![doc(html_favicon_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]

//! Single-threaded, thread-aware application runtime.
//!
//! Arty is being developed as a small runtime whose foundational contracts live in
//! [`arty_core`]. Its public surface is intentionally limited while those contracts are being
//! established.
//!
//! # Features
//!
//! - **`time`** - Exposes time primitives through [`time`].
//! - **`test-util`** - Exposes `time::ClockControl` when `time` is also enabled.
//!
//! # Project policies
//!
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/DESIGN.md)
//! - [I/O](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md)
//! - [Panics](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md)
//! - [Stabilization](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/STABILIZATION.md)

/// Foundational runtime and thread-awareness types.
pub mod core {
    #[doc(inline)]
    pub use thread_aware_core::*;
}

/// Time primitives for the runtime.
#[cfg(feature = "time")]
#[cfg_attr(docsrs, doc(cfg(feature = "time")))]
pub mod time {
    #[doc(inline)]
    pub use tick::{
        Clock, Delay, Error, FutureExt, PeriodicTimer, Result, SimpleClock, Stopwatch,
        SystemTimeExt, Timeout,
    };

    #[cfg(feature = "test-util")]
    #[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
    #[doc(inline)]
    pub use tick::ClockControl;
}
