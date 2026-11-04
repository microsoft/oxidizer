// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Hosts I/O drivers for the unstable Arty runtime.
//!
//! This crate supports [`arty`](https://docs.rs/arty). For applications and
//! libraries, use `arty` instead of depending on this crate directly.
//!
//! Arty uses these public items across a crate boundary. The items do not form
//! a standalone stable API.
//!
//! # Quick start
//!
//! Create one [`IoRuntime`] for the runtime. Clone it for each worker, then
//! create a [`DriverHost`] with the worker's thread coordinate:
//!
//! ```
//! # use std::task::Waker;
//! # use std::time::Duration;
//! # use arty_io_host::IoRuntime;
//! # use observed::Sink;
//! # use thread_aware::ThreadBuilder;
//! # fn main() -> Result<(), arty_io_core::DriverError> {
//! // One value owns the shared context registry and telemetry sink.
//! let runtime = IoRuntime::new(Sink::noop());
//! let worker = ThreadBuilder::default().build(std::thread::current().id());
//!
//! // Each worker creates its host on the thread that its coordinate identifies.
//! let host = runtime.clone().driver_host(worker, Waker::noop().clone())?;
//!
//! // The worker loop supplies the available wait time.
//! host.execute_cycle(Duration::ZERO)?;
//! # Ok(())
//! # }
//! ```

pub use driver_host::DriverHost;
pub use runtime::IoRuntime;

mod driver_host;
mod runtime;
mod telemetry;
