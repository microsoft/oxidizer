// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Unstable I/O driver hosting utilities for the Arty runtime.
//!
//! This crate is implementation infrastructure for [`arty`](https://docs.rs/arty).
//! Applications and libraries should use `arty` instead of depending on this
//! crate directly.
//!
//! The items are public because Arty consumes them across a crate boundary.
//! They do not form a standalone stability or compatibility promise.

pub use driver_host::DriverHost;
pub use runtime::IoRuntime;

mod driver_host;
mod runtime;
mod telemetry;
