// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Contracts for integrating I/O drivers with an async runtime.
//!
//! This crate defines the types shared by runtimes and drivers that evolve independently. It
//! provides neither a runtime nor an I/O implementation.
//!
//! # Core types
//!
//! - [`IoContext`] is the consumer-facing handle that selects a [`DriverProvider`].
//! - [`DriverProvider`] creates one [`Driver`] and [`IoContext`] for each runtime worker.
//! - [`Driver`] is the thread-local adapter between a worker and an I/O subsystem.
//! - [`ProviderOptions`] and [`DriverOptions`] carry runtime facilities during registration.
//! - [`DriverHandle`] lets drivers discover peers registered on the same worker.
//! - [`SystemTaskSpawner`] runs blocking system work on runtime-owned threads.
//! - [`ShutdownError`] reports a failure to complete graceful shutdown.
//!
//! # Registration
//!
//! A runtime registers a driver when an [`IoContext`] type is first requested. It calls
//! [`IoContext::provider`] once, then clones and relocates the provider for each active worker.
//! Each relocated provider is consumed by [`DriverProvider::create`], which returns the worker's
//! driver and context.
//!
//! [`DriverOptions`] identifies the worker, supplies runtime facilities, and contains handles to
//! drivers registered earlier on that worker. After storing the new driver, the runtime calls
//! [`Driver::on_peer_registered`] on those earlier drivers. This gives both the new driver and its
//! peers an opportunity to exchange independently owned shared state.
//!
//! The first context request completes after every active worker has created its driver and
//! context. Later requests clone the context stored alongside the driver on the calling worker.
//! Registration is infallible at the type level: a provider or peer callback panics if
//! registration cannot be completed.
//!
//! # Driving I/O
//!
//! A runtime owns each driver and invokes its methods only on the worker that created it. Drivers
//! are therefore not required to implement [`Send`] or [`Sync`].
//!
//! [`Driver::process_completions`] processes pending work and optionally waits for more.
//! [`Driver::waker`] interrupts the current or next blocking wait. Contexts may move between
//! workers and may outlive their associated driver.
//!
//! State reachable from a context, waker, background thread, or operating-system callback must be
//! owned independently of the driver and synchronized as necessary. State used only by the owning
//! worker may remain directly in the driver.
//!
//! # Shutdown
//!
//! [`Driver::shutdown`] consumes the driver, closes admission to new operations, and waits for
//! active work to drain. It returns [`ShutdownError`] when graceful cleanup cannot be completed.
//! The runtime keeps [`SystemTaskSpawner`] available until shutdown returns.
//!
//! A driver must be safe to drop at every point in its lifecycle, including during unwinding and
//! after a shutdown error. Contexts remain valid after shutdown but reject new operations.
//!
//! # Example
//!
//! The [single-thread runtime example](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/single_thread_runtime/main.rs)
//! demonstrates lazy registration of two context types and same-worker driver discovery.
//!
//! # Project documents
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)

mod driver;
mod driver_handle;
mod driver_options;
mod io_context;
mod provider;
mod provider_options;
mod shutdown_error;
mod system_task_spawner;

pub use driver::Driver;
pub use driver_handle::DriverHandle;
pub use driver_options::DriverOptions;
pub use io_context::IoContext;
pub use provider::DriverProvider;
pub use provider_options::ProviderOptions;
pub use shutdown_error::ShutdownError;
pub use system_task_spawner::{SystemTask, SystemTaskSpawner};
