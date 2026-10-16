// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Unstable I/O driver hosting utilities for the Arty runtime.
//!
//! This crate is implementation infrastructure for [`arty`](https://docs.rs/arty).
//! Applications and libraries should use `arty` instead of depending on this
//! crate directly.
//!
//! The items are public because Arty consumes them across a crate boundary.
//! They do not form a standalone stability or compatibility promise.
//!
//! # Quick start
//!
//! Clone the runtime-wide registry for each worker, relocate the clone onto
//! that worker, and then create its driver host:
//!
//! ```
//! # use std::task::Waker;
//! # use std::time::Duration;
//! # use arty_io_host::IoRuntime;
//! # use observed::Sink;
//! # use thread_aware::{ThreadAware, ThreadBuilder};
//! # fn main() -> Result<(), arty_io_core::DriverError> {
//! // One runtime-wide value owns the shared context registry and telemetry sink.
//! let runtime = IoRuntime::new(Sink::noop());
//! let worker = ThreadBuilder::default().build(std::thread::current().id());
//!
//! // Each worker receives a clone that must be relocated on the destination
//! // thread before its worker-local host is created.
//! let mut worker_runtime = runtime.clone();
//! worker_runtime.relocate(None, &worker);
//! let host = worker_runtime.driver_host(Waker::noop().clone());
//!
//! // The runtime calls this from its worker loop with the available wait time.
//! host.execute_cycle(Duration::ZERO)?;
//! # Ok(())
//! # }
//! ```

pub use driver_host::DriverHost;
pub use runtime::IoRuntime;

mod driver_host;
mod runtime;
mod telemetry;
