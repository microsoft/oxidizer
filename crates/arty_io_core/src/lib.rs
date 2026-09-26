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
//! Before entering or scheduling a native wait, the driver calls [`Cycle::start_work`] with an
//! interruption waker whose signal remains latched until the wait observes it. It keeps the
//! returned [`PendingWork`] until the work ends and publishes any results before completing or
//! dropping the handle. Both actions notify the runtime.
//!
//! ## Primary and secondary drivers
//!
//! A worker has at most one [`Primary`](DriverRole::Primary) driver, assigned only to a provider
//! that opts in through [`DriverProvider::CAN_BE_PRIMARY`]. It may block the worker only when
//! [`Cycle::can_block`] permits it.
//!
//! [`Secondary`](DriverRole::Secondary) drivers must not block the worker. They may schedule
//! background waits represented by [`PendingWork`]; indefinite waits require independent
//! execution capacity.
//!
//! # Runtime responsibilities
//!
//! The runtime clones and relocates providers to their workers, assigns driver roles, and
//! supplies [`DriverOptions`]. It completes a non-blocking, zero-wait initialization cycle
//! before publishing a context or notifying peers through [`Driver::on_peer_registered`].
//! It also supplies a [`SystemTaskSpawner`] for blocking system work.
//!
//! Each logical cycle uses a shared time snapshot and wait bound. The runtime invokes
//! secondaries before the primary and implements [`PendingWorkTracker`] to register work,
//! latch interruption, and track completion. After the primary returns, it interrupts remaining
//! waits and waits for every pending-work handle before advancing. If there is no primary,
//! the runtime retains responsibility for parking the worker.
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
//! The [single-thread runtime example] demonstrates registration and peer discovery. Its sample
//! drivers perform no I/O and use a no-op tracker; a runtime serving native I/O must implement
//! the coordination described above.
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)
//!
//! [single-thread runtime example]: https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core/examples/single_thread_runtime

mod cycle;
mod driver;
mod driver_error;
mod driver_handle;
mod driver_options;
mod driver_role;
mod io_context;
mod pending_work;
mod pending_work_tracker;
mod provider;
mod provider_options;
mod shutdown_error;
mod system_task_spawner;

pub use cycle::Cycle;
pub use driver::Driver;
pub use driver_error::DriverError;
pub use driver_handle::DriverHandle;
pub use driver_options::DriverOptions;
pub use driver_role::DriverRole;
pub use io_context::IoContext;
pub use pending_work::PendingWork;
pub use pending_work_tracker::PendingWorkTracker;
pub use provider::DriverProvider;
pub use provider_options::ProviderOptions;
pub use shutdown_error::ShutdownError;
pub use system_task_spawner::{SystemTask, SystemTaskSpawner};
