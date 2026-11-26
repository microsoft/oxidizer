// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Connecting I/O libraries to Arty.
//!
//! An I/O context gives an I/O library access to its driver.
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
//! A library that provides I/O endpoints should accept an [`Io`] from the
//! application. Its public API should expose the endpoint or client, not the
//! library's specific [`IoContext`](arty_io_core::IoContext).
//!
//! The library uses `Io` to obtain its internal context. This keeps the
//! application independent of the I/O implementation's context and lets the
//! library relocate its context with the Arty worker. This example requires
//! `macros`, which enables `rt`:
//!
//! ```
//! # use std::task::Waker;
//! # use arty::core::{Thread, ThreadAware};
//! # use arty::io::Io;
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
//! #     fn new((): Self::ContextOptions) -> Self {
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
//! # struct ExampleEndpoint {
//! #     context: ExampleIo,
//! # }
//! # impl ExampleEndpoint {
//! #     fn new(io: &Io) -> Self {
//! #         Self { context: io.context::<ExampleIo>(&()) }
//! #     }
//! #
//! #     fn submit(&self, request: &str) {
//! #         self.context.submit(request);
//! #     }
//! # }
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! #[arty::main]
//! async fn main(cx: Context) {
//!     let endpoint = ExampleEndpoint::new(cx.io());
//!     endpoint.submit("request");
//! }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! `ExampleEndpoint` represents the public endpoint API. `ExampleIo` is the
//! implementation detail that connects it to Arty.
//!
//! # Library configuration
//!
//! An I/O context can require inputs such as a memory provider, clock, or
//! credentials. The I/O library should accept these inputs through its own
//! builder-style API. The builder should normally accept application-level
//! values, such as `OpaqueMemory`, rather than expose the concrete
//! [`IoContext::ContextOptions`](arty_io_core::IoContext::ContextOptions)
//! type. When the endpoint is built, the library assembles private context
//! options and calls [`Io::context`].
//!
//! The builder should store the [`Io`] and other dependencies separately from
//! the endpoint. This allows the application to select services for the whole
//! process while the I/O library decides how its context uses them.
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
