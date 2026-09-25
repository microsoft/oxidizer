// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Contracts for integrating I/O drivers with an async runtime.
//!
//! This crate defines the types shared by runtimes and independently versioned drivers. It
//! provides neither a runtime nor an I/O implementation.
//!
//! # Core types
//!
//! - [`IoContext`] selects a [`DriverProvider`].
//! - [`DriverProvider`] creates one [`Driver`] and context per runtime worker.
//! - [`DriverRole`] identifies the worker-blocking primary and non-blocking secondaries.
//! - [`Cycle`] supplies a shared time snapshot, wait bound, and [`Coordinator`].
//! - [`DriverOptions`] supplies per-worker construction facilities and peer handles.
//! - [`SystemTaskSpawner`] runs blocking system work outside async workers.
//! - [`DriverError`] and [`ShutdownError`] report infrastructure and cleanup failures.
//!
//! # Registration
//!
//! The first request for an [`IoContext`] creates its provider and initializes a driver/context
//! pair on every active worker. Before publishing a context, the runtime assigns the driver's
//! role and runs an initial zero-wait cycle. The new driver sees earlier drivers through
//! [`DriverOptions::drivers`]; earlier drivers receive the new driver's handle through
//! [`Driver::on_peer_registered`]. Later requests reuse the registration.
//!
//! # Driving I/O
//!
//! A runtime invokes secondary drivers first and the primary last. Every driver receives the same
//! [`Cycle::max_wait`]. A primary may block its worker for that duration. A secondary must return
//! promptly and may use the duration only for a wait scheduled on a background thread.
//!
//! Drivers create non-cloneable [`CoordinationToken`] values with [`Cycle::start_work`] and attach
//! native-wait callbacks with [`CoordinationToken::on_interrupted`]. The runtime waits for every
//! token after the primary returns and before starting the next cycle. A driver calls
//! [`CoordinationToken::work_ready`] after publishing work, or drops the token if its wait ended
//! without work.
//!
//! # Shutdown
//!
//! [`Driver::shutdown`] consumes the driver, closes admission, and blocks until cleanup completes
//! or fails. Contexts remain valid as closed handles. A driver must not depend on work that can
//! run only after its shutdown returns.
//!
//! # Project documents
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)
//! - [Completion coordination (exploratory)](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/COMPLETION_COORDINATION.md)

mod coordinator;
mod cycle;
mod driver;
mod driver_error;
mod driver_handle;
mod driver_options;
mod driver_role;
mod io_context;
mod provider;
mod provider_options;
mod shutdown_error;
mod system_task_spawner;

pub use coordinator::{CoordinationToken, Coordinator};
pub use cycle::Cycle;
pub use driver::Driver;
pub use driver_error::DriverError;
pub use driver_handle::DriverHandle;
pub use driver_options::DriverOptions;
pub use driver_role::DriverRole;
pub use io_context::IoContext;
pub use provider::DriverProvider;
pub use provider_options::ProviderOptions;
pub use shutdown_error::ShutdownError;
pub use system_task_spawner::{SystemTask, SystemTaskSpawner};
