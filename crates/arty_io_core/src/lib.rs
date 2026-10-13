// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Stable contracts for integrating I/O drivers into thread-aware runtimes.
//!
//! Drivers provide I/O; runtimes provide scheduling. This crate defines
//! the interfaces between them, but provides neither a runtime nor an I/O implementation.
//!
//! # How drivers work
//!
//! Applications access a driver's I/O operations through an [`IoContext`]. It creates a
//! worker-local [`Driver`] for each runtime worker. The driver processes submissions and
//! completions in bounded calls to [`Driver::execute_cycle`].
//!
//! ```
//! use std::task::Waker;
//!
//! use arty_io_core::{
//!     Cycle, Driver, DriverError, DriverInstance, DriverOptions, DriverRole, IoContext,
//! };
//! use thread_aware_core::{Thread, ThreadAware};
//!
//! #[derive(Clone)]
//! struct Context;
//!
//! struct ExampleDriver;
//!
//! impl ThreadAware for Context {
//!     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
//! }
//!
//! impl IoContext for Context {
//!     type ContextOptions = ();
//!     type Driver = ExampleDriver;
//!
//!     fn context((): Self::ContextOptions) -> Self {
//!         Self
//!     }
//!
//!     fn driver(
//!         &mut self,
//!         options: DriverOptions,
//!     ) -> Result<DriverInstance<Self::Driver>, DriverError> {
//!         let role = options
//!             .allowed_roles()
//!             .first()
//!             .copied()
//!             .ok_or_else(|| DriverError::from_message("no supported driver role"))?;
//!         Ok(DriverInstance::new(ExampleDriver, role))
//!     }
//! }
//!
//! impl Driver for ExampleDriver {
//!     fn waker(&self) -> Waker {
//!         Waker::noop().clone()
//!     }
//!
//!     fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
//!         Ok(())
//!     }
//!
//!     fn shutdown(self) -> Result<(), DriverError> {
//!         Ok(())
//!     }
//! }
//!
//! # fn assert_context<C: IoContext>() {}
//! # assert_context::<Context>();
//! ```
//!
//! ## Primary and secondary drivers
//!
//! A worker has at most one [`Primary`](DriverRole::Primary) driver. [`DriverOptions`] lists the
//! roles available during creation, and [`DriverInstance`] returns the selected role. A primary
//! may wait on the worker for up to [`Cycle::max_wait`]; a zero wait bound means no waiting.
//!
//! [`Secondary`](DriverRole::Secondary) drivers must not block the worker. They may schedule
//! background waits on independent execution capacity.
//!
//! # Runtime responsibilities
//!
//! The runtime clones and relocates contexts to its workers and supplies the acceptable roles
//! through [`DriverOptions`]. The context selects one of those roles and returns it in a
//! [`DriverInstance`]. The runtime completes a non-blocking, zero-wait initialization cycle before
//! publishing a context.
//! It also supplies an owner waker for requesting another cycle. Each driver supplies a waker that
//! interrupts its pending completion wait.
//!
//! Each logical cycle uses a shared wait bound. The runtime invokes secondaries before the
//! primary. If there is no primary, the runtime retains responsibility for parking the worker.
//!
//! # Shutdown
//!
//! The runtime stops normal cycles and calls [`Driver::shutdown`] for every driver, continuing
//! after a [`DriverError`]. Each driver closes admission and drains its resources within a
//! bounded wait, independently of other drivers on the same worker. Contexts remain valid as
//! closed handles.
//!
//! See the [design notes] for the lifecycle and ownership model.
//!
//! [design notes]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md

mod cycle;
mod driver;
mod driver_error;
mod driver_instance;
mod driver_options;
mod driver_role;
mod io_context;

pub use cycle::Cycle;
pub use driver::Driver;
pub use driver_error::DriverError;
pub use driver_instance::DriverInstance;
pub use driver_options::DriverOptions;
pub use driver_role::DriverRole;
pub use io_context::IoContext;
