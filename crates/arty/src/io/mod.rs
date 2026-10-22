// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! I/O drivers for Arty runtimes.
//!
//! Arty hosts I/O drivers but does not implement I/O itself. An I/O provider
//! supplies an [`IoContext`](arty_io_core::IoContext) and
//! [`Driver`](arty_io_core::Driver); Arty runs the driver and gives tasks access
//! to the context. Providers remain free to use any protocol, library, or
//! operating-system facility.
//!
//! # Example
//!
//! Obtain an I/O context from the task's built-in services, then use the
//! operations exposed by that context:
//!
//! ```
//! # use std::task::Waker;
//! # use arty::core::{Thread, ThreadAware};
//! # use arty::task::Builtins;
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
//! async fn main(cx: Builtins) {
//!     let io = cx.io().context::<ExampleIo>(&());
//!     io.submit("request");
//! }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! `ExampleIo` stands in for a provider's context; real contexts expose their
//! own operations.
//!
//! # Using multiple providers
//!
//! A runtime can host multiple context types, with one driver of each type per
//! worker. The first context type requested becomes the blocking
//! [`Primary`](arty_io_core::DriverRole::Primary); the others run as non-blocking
//! [`Secondary`](arty_io_core::DriverRole::Secondary) drivers.
//!
//! Request a context with [`Io::context`]. Each concrete context type is
//! registered once per runtime and reused on later requests.

mod current_host;
mod handle;

pub(crate) use current_host::RuntimeDriverHost;
pub use handle::Io;
