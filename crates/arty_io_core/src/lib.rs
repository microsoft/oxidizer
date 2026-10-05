// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Stable contracts for integrating I/O drivers into thread-aware runtimes.
//!
//! Drivers provide I/O; runtimes provide scheduling and work coordination. This crate defines
//! the interfaces between them, but provides neither a runtime nor an I/O implementation.
//!
//! # How drivers work
//!
//! Applications access a driver's I/O operations through an [`IoContext`]. Its [`DriverProvider`]
//! creates a context and a worker-local [`Driver`] for each runtime worker. The driver processes
//! submissions and completions in bounded calls to [`Driver::execute_cycle`].
//!
//! Drivers expose a [`Driver::waker`] for interrupting pending completion waits. Wakes are
//! latched until the wait observes them, and the waker remains safe after the driver is dropped.
//!
//! ## Primary and secondary drivers
//!
//! A worker has at most one [`Primary`](DriverRole::Primary) driver. The runtime grants the
//! primary permission through [`DriverOptions::role`], and the provider returns the role selected
//! by the driver in [`DriverInstance`]. A primary may wait on the worker for up to
//! [`Cycle::max_wait`]; a zero wait bound means no waiting.
//!
//! [`Secondary`](DriverRole::Secondary) drivers must not block the worker. They may coordinate
//! with other drivers or continuously process completions on a driver-owned background thread.
//!
//! # Runtime responsibilities
//!
//! The runtime clones and relocates providers to their workers, grants role permission through
//! [`DriverOptions`], and receives a [`DriverInstance`] containing the selected role. It
//! completes a non-blocking, zero-wait initialization cycle before publishing a context.
//! It also supplies a [`SystemTaskSpawner`] for blocking system work.
//!
//! Each logical cycle supplies a wait bound. The runtime invokes secondaries before the primary.
//! Drivers own coordination for work that continues beyond their worker-local call. If there is
//! no primary, the runtime retains responsibility for parking the worker.
//!
//! # Shutdown
//!
//! The runtime stops normal cycles and calls [`Driver::shutdown`] for every driver, continuing
//! after a [`ShutdownError`]. Each driver closes admission and drains its resources within a
//! bounded wait, independently of other drivers on the same worker. Contexts remain valid as
//! closed handles.
//!
//! # Example and reference
//!
//! The [single-thread runtime example] demonstrates registration and driver roles. Its sample
//! drivers perform no I/O and use no-op wakers; a runtime serving native I/O must implement the
//! coordination described above.
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)
//!
//! [single-thread runtime example]: https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core/examples/single_thread_runtime

mod cycle;
mod driver;
mod driver_error;
mod driver_instance;
mod driver_options;
mod driver_role;
mod io_context;
mod provider;
mod provider_options;
mod shutdown_error;
mod system_task_spawner;

pub use cycle::Cycle;
pub use driver::Driver;
pub use driver_error::DriverError;
pub use driver_instance::DriverInstance;
pub use driver_options::DriverOptions;
pub use driver_role::DriverRole;
pub use io_context::IoContext;
pub use provider::DriverProvider;
pub use provider_options::ProviderOptions;
pub use shutdown_error::ShutdownError;
pub use system_task_spawner::{SystemTask, SystemTaskSpawner};
