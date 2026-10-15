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
//! Applications access I/O through an [`IoContext`]. The runtime creates one worker-local
//! [`Driver`] for that context on each async worker. The driver submits I/O and processes
//! completed operations in [`Driver::execute_cycle`].
//!
//! ```
//! use std::task::Waker;
//!
//! use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
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
//!     fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
//!         let _role = options.role();
//!         Ok(ExampleDriver)
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
//! An async worker has at most one [`Primary`](DriverRole::Primary) driver. [`DriverOptions`]
//! provides the role assigned during creation. A primary may wait for I/O for up to
//! [`Cycle::max_wait`].
//!
//! The runtime calls [`Secondary`](DriverRole::Secondary) drivers with
//! `Cycle::max_wait() == Duration::ZERO`. A secondary processes completions that are already ready
//! or returns immediately if another thread handles its notifications. A secondary must wake its
//! async worker when another cycle is needed.
//!
//! # Runtime responsibilities
//!
//! The runtime clones and relocates contexts to its async workers and supplies the assigned role
//! through [`DriverOptions`]. Driver creation completes all setup before the context is published.
//!
//! The runtime invokes secondaries before the primary. If there is no primary, the runtime waits
//! for work itself. The owner waker in [`DriverOptions`] wakes the async worker, while
//! [`Driver::waker`] makes a waiting driver return promptly.
//!
//! A driver creation or cycle error is fatal. The runtime starts shutdown on every async worker
//! and calls `shutdown` on every driver that was created successfully, including the driver whose
//! cycle failed.
//!
//! # Shutdown
//!
//! When an async worker shuts down, the runtime stops calling `execute_cycle` and synchronously
//! calls [`Driver::shutdown`] for every driver owned by that worker. Each driver rejects new
//! operations and uses a finite shutdown limit. A returned [`DriverError`] does not prevent the
//! runtime from attempting to shut down the remaining drivers.
pub use cycle::Cycle;
pub use driver::Driver;
pub use driver_error::DriverError;
pub use driver_options::DriverOptions;
pub use driver_role::DriverRole;
pub use io_context::IoContext;

mod cycle;
mod driver;
mod driver_error;
mod driver_options;
mod driver_role;
mod io_context;
