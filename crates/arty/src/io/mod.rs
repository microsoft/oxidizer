// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Connecting I/O libraries to Arty.
//!
//! An I/O context gives your code access to an I/O library.
//! A driver processes that library's I/O requests.
//! Related crates and third-party libraries connect to Arty through `arty_io_core`.
//! They supply an [`IoContext`](arty_io_core::IoContext) and
//! [`Driver`](arty_io_core::Driver).
//! Each worker creates a driver when it first requests that context.
//! A worker has at most one driver for each context type.
//!
//! I/O libraries must add `arty_io_core` as a direct dependency.
//! Applications add Arty and the selected I/O library.
//!
//! # Use an I/O library
//!
//! Obtain a library's context from [`Context::io`](crate::task::Context::io).
//! Then use the operations that the library provides. This example requires
//! `macros`, which enables `rt`:
//!
//! ```
//! # use std::task::Waker;
//! # use arty::core::{Thread, ThreadAware};
//! # use arty::task::Context;
//! # use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
//! # #[derive(Clone)]
//! # struct ExampleIo;
//! # impl ExampleIo {
//! #     fn submit(&self, _request: &str) {}
//! # }
//! # impl ThreadAware for ExampleIo {
//! #     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
//! # }
//! # impl IoContext for ExampleIo {
//! #     type ContextOptions = ();
//! #     type Driver = ExampleDriver;
//! #
//! #     fn context((): Self::ContextOptions) -> Self {
//! #         Self
//! #     }
//! #
//! #     fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
//! #         Ok(ExampleDriver)
//! #     }
//! # }
//! # struct ExampleDriver;
//! # impl Driver for ExampleDriver {
//! #     fn waker(&self) -> Waker {
//! #         Waker::noop().clone()
//! #     }
//! #
//! #     fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
//! #         Ok(())
//! #     }
//! #
//! #     fn shutdown(self) -> Result<(), DriverError> {
//! #         Ok(())
//! #     }
//! # }
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! #[arty::main]
//! async fn main(cx: Context) {
//!     let io = cx.io().context::<ExampleIo>(&());
//!     io.submit("request");
//! }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! `ExampleIo` represents an I/O library's context.
//!
//! # Driver roles
//!
//! The driver for the first context type registered in the runtime is
//! [`Primary`](arty_io_core::DriverRole::Primary). It can wait for I/O.
//! Drivers for later context types are
//! [`Secondary`](arty_io_core::DriverRole::Secondary). They must not wait for I/O.
//! [`Io::context`] creates each context type once per runtime.
//! The first call that creates the context determines its options.
//! Each worker creates its driver when it first requests that context.
//!
//! See the [I/O guide](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md)
//! for cross-thread access, failure, and shutdown behavior.

mod driver_host;
mod handle;

pub(crate) use driver_host::RuntimeDriverHost;
pub use handle::Io;
