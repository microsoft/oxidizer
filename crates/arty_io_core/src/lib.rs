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
//! - [`Driver`] is the adapter between one worker and an I/O subsystem.
//! - [`DriverContext`] associates a requested context type with its provider.
//! - [`DriverProvider`] creates and connects the per-worker adapters for a driver.
//! - [`DriverInit`] describes the worker and runtime facilities available during creation.
//! - [`Parker`] lets a driver provide a waiting point for the worker.
//! - [`SystemTasks`] lets a driver delegate blocking system work to the runtime.
//!
//! Registration and driver placement are runtime behavior, not part of this crate. Keeping those
//! policies outside the contract allows the runtime and drivers to evolve independently.
//!
//! # Example
//!
//! The [fixed two-thread runtime example](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs)
//! starts both worker threads before `get_context::<SampleContext>()` uses the context type to
//! inject its associated driver.
//!
//! # Project documents
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)

mod context;
mod driver;
mod init;
mod parker;
mod provider;
mod system_tasks;

pub use context::DriverContext;
pub use driver::Driver;
pub use init::DriverInit;
pub use parker::Parker;
pub use provider::DriverProvider;
pub use system_tasks::{SystemTask, SystemTasks};
