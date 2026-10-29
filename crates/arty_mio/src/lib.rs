// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

//! Mio I/O integration for the Arty runtime.
//!
//! Each [`MioDriver`] owns a Mio [`Poll`](mio::Poll) and its driver-side
//! lifecycle. A primary driver polls on the Arty worker. A secondary driver
//! uses a helper thread to wait for Mio events and wake the Arty worker.
//!
//! [`MioContext`] selects the worker route for a new source. It registers the
//! source on the calling thread through a cloned Mio registry. The driver
//! creates the clone with `mio::Registry::try_clone`. [`MioContext::register`]
//! returns one [`Registered`] value that owns the source.
//!
//! [`Registered`] provides ordinary shared and exclusive references to the
//! source. It does not put the source behind a mutex. Read and write readiness
//! use separate poll methods that return `()`. Readiness is a hint after an I/O
//! operation returns `io::ErrorKind::WouldBlock`. Driver shutdown closes
//! readiness, but the `Registered` owner retains and drops the source.
//!
//! # Example
//!
//! ```no_run
//! use std::io;
//!
//! use arty::task::Builtins;
//! use arty_mio::{MioContext, MioOptions};
//! use mio::Interest;
//!
//! async fn receive(builtins: &Builtins) -> io::Result<()> {
//!     let io = builtins.io().context::<MioContext>(&MioOptions::default());
//!     let address = "127.0.0.1:0".parse().expect("the example address is valid");
//!     let socket = mio::net::UdpSocket::bind(address)?;
//!     let mut registration = io.register(socket, Interest::READABLE)?;
//!     let mut buffer = [0; 1_024];
//!
//!     loop {
//!         match registration
//!             .source_mut()
//!             .map_err(io::Error::other)?
//!             .recv(&mut buffer)
//!         {
//!             Ok(length) => {
//!                 println!("received {length} bytes");
//!                 return Ok(());
//!             }
//!             Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
//!                 std::future::poll_fn(|context| registration.poll_read_ready(context))
//!                     .await
//!                     .map_err(io::Error::other)?;
//!             }
//!             Err(error) => return Err(error),
//!         }
//!     }
//! }
//! ```

pub use context::MioContext;
pub use driver::MioDriver;
pub use error::RegistrationClosed;
pub use options::MioOptions;
pub use registration::Registered;

mod context;
mod driver;
mod error;
mod options;
mod registration;
mod telemetry;
mod worker;
