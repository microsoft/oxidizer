// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Contracts between I/O drivers and thread-aware runtimes.
//!
//! Drivers provide I/O. Runtimes provide scheduling. This crate defines the interfaces between
//! them. It does not provide a runtime or an I/O implementation.
//!
//! # How drivers work
//!
//! Applications access I/O through an [`IoContext`]. On each async worker, the runtime creates
//! one local [`Driver`] for that context. The driver submits I/O and processes completed
//! operations in [`Driver::execute_cycle`].
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
//! contains the assigned role. A primary can wait for I/O for up to
//! [`Cycle::max_wait`].
//!
//! The runtime calls [`Secondary`](DriverRole::Secondary) drivers with
//! `Cycle::max_wait() == Duration::ZERO`. A secondary either processes ready completions or
//! returns immediately if another thread handles its notifications. It must wake its async worker
//! when another cycle is necessary.
//!
//! # Runtime responsibilities
//!
//! The runtime registers each concrete context type once. When a worker first retrieves a context,
//! the runtime clones it. The runtime then calls
//! [`ThreadAware::relocate`](thread_aware_core::ThreadAware::relocate) and
//! [`IoContext::driver`]. The relocation destination and [`DriverOptions::thread`] identify the
//! same worker. [`DriverOptions`] also contains the assigned role.
//!
//! The runtime calls `driver` at most once per registration and async worker, not once per clone.
//! Application operations require successful initialization. Implementations can reject duplicate
//! driver creation. They can also reject operations on uninitialized contexts. Clones of
//! initialized contexts remain usable without another driver call. Later relocation is optional
//! under [`ThreadAware`](thread_aware_core::ThreadAware).
//!
//! The runtime invokes secondaries before the primary. If there is no primary, the runtime waits
//! for work itself. The owner waker in [`DriverOptions`] wakes the async worker.
//! [`Driver::waker`] makes a waiting driver return promptly.
//!
//! A driver creation or cycle error is fatal. The runtime starts shutdown on every async worker.
//! It calls `shutdown` on every driver that was created successfully, including the driver whose
//! cycle failed. Shutdown closes each context to new operations. Contexts returned by other workers
//! must then reject new operations.
//!
//! # Shutdown
//!
//! When an async worker shuts down, the runtime stops calling `execute_cycle`. It synchronously
//! calls [`Driver::shutdown`] for every driver that the worker owns. Each driver rejects new
//! operations and uses a finite shutdown limit. A returned [`DriverError`] does not stop the
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
