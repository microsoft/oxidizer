// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! External I/O contexts and worker-local drivers.
//!
//! Companion and third-party crates can integrate through `arty_io_core`. An
//! I/O provider supplies an [`IoContext`](arty_io_core::IoContext) and
//! [`Driver`](arty_io_core::Driver). Arty lazily hosts at most one driver of
//! each registered context type on each worker.
//!
//! Provider implementations must add `arty_io_core` as a direct dependency.
//! Applications add Arty and the selected provider crate.
//!
//! # Use a provider
//!
//! Obtain a provider context from [`Context::io`](crate::task::Context::io),
//! then use the operations that the provider exposes. This example requires
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
//! `ExampleIo` represents a provider context.
//!
//! # Driver roles
//!
//! The first context type requested becomes the blocking
//! [`Primary`](arty_io_core::DriverRole::Primary). The other types run as
//! non-blocking [`Secondary`](arty_io_core::DriverRole::Secondary) drivers.
//! [`Io::context`] registers each concrete context type once per runtime.
//! Workers initialize their driver when they first request that context.
//!
//! See the [I/O guide](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md)
//! for cross-thread access, failure, and shutdown behavior.

mod driver_host;
mod handle;

pub(crate) use driver_host::RuntimeDriverHost;
pub use handle::Io;
