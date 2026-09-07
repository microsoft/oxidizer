// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Stable contracts for integrating external I/O drivers with the Arty runtime.
//!
//! The runtime hosts drivers supplied by libraries and applications rather than depending on one
//! I/O implementation. This crate contains the small vocabulary both sides share:
//!
//! - [`Driver`] is one worker's adapter to an I/O subsystem.
//! - [`DriverProvider`] creates and connects a driver's per-worker adapters.
//! - [`DriverInit`] describes the worker and runtime facilities available during creation.
//! - [`Parker`] lets a driver provide the worker's waiting point.
//! - [`BlockingTaskSpawner`] lets a driver offload work that is allowed to block.
//!
//! Registration and driver placement are runtime behavior, not part of this crate. Keeping those
//! policies outside the contract allows the runtime and drivers to evolve independently.
//!
//! # Project documents
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)

mod blocking;
mod driver;
mod init;
mod parker;
mod provider;
mod shutdown;

pub use blocking::{BlockingTask, BlockingTaskSpawner};
pub use driver::Driver;
pub use init::{DriverInit, WaitingPoint};
pub use parker::Parker;
pub use provider::DriverProvider;
pub use shutdown::Shutdown;
pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
