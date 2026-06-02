// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_hyper/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_hyper/favicon.ico")]

//! Hyper-based HTTP transport.
//!
//! # Internal implementation detail
//!
//! This crate is an internal implementation detail of the `SDK`. It is not part
//! of the public API surface, must not be re-exported, and offers no stability
//! guarantees: anything may change in any release, including patch releases.
//!
//! Scope is narrow: just the transport that issues HTTP/1.1 or HTTP/2 requests
//! over `TLS` (or plain-text). No higher-level pipeline, retry, or caching.
//!
//! The entry points are:
//!
//! - [`HyperTransportBuilder`]: builds a transport from a user-supplied
//!   [`Connect`] service and a [`fetch_options::TransportOptions`].
//! - [`HyperTransport`]: the type-erased [`RequestHandler`] produced by
//!   [`HyperTransportBuilder::build`].
//!
//! The runtime is supplied by the caller via an [`anyspawn::Spawner`].
//!
//! [`RequestHandler`]: http_extensions::RequestHandler

mod builder;
mod connection;
mod error_labels;
mod recoverability;
mod telemetry;
mod timer;
mod tls;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod testing;

pub use builder::{HyperTransport, HyperTransportBuilder};
pub use connection::{Connect, HyperIo};
pub use tls::TlsBackend;
