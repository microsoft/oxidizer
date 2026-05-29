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
//! This crate is an **internal implementation detail** of the `SDK`. It is not
//! part of the public API surface and must not be re-exported from any other
//! crate. Consumers should depend on the higher-level `SDK` crates instead of
//! taking a direct dependency on `fetch_hyper`, and `SDK` crates that do depend
//! on it must keep its types out of their own public APIs (no `pub use`,
//! no types appearing in public function signatures, trait bounds, or
//! associated types).
//!
//! No stability guarantees are offered: items may be added, renamed, removed,
//! or have their semantics changed in any release — including patch releases —
//! without notice.
//!
//! Narrow scope: just the transport that issues HTTP/1.1 or HTTP/2 requests
//! over `TLS` (or plain-text). No higher-level pipeline, retry, caching, etc.
//!
//! The entry points are:
//!
//! - [`HyperTransportBuilder`]: generic over a user-supplied [`Connect`]
//!   service. Exposes setters for the few knobs driving our own logic plus a
//!   [`configure_hyper`](HyperTransportBuilder::configure_hyper) escape
//!   hatch for `hyper`'s own builder.
//! - [`HyperTransport`]: the type-erased [`RequestHandler`] produced
//!   by [`HyperTransportBuilder::build`].
//!
//! The runtime is supplied entirely by the caller via an
//! [`anyspawn::Spawner`] together with any service implementing [`Connect`].
//!
//! [`RequestHandler`]: http_extensions::RequestHandler

mod builder;
mod connection;
mod error_labels;
mod options;
mod recoverability;
mod telemetry;
mod timer;
mod tls;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod testing;

pub use builder::{HyperTransport, HyperTransportBuilder};
pub use connection::{Connect, HyperIo};
pub use options::{ConnectionLifetime, RequestFilter};
pub use telemetry::ConnectionInfo;
pub use tls::TlsBackend;
