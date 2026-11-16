// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
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
//! # TLS failure diagnostics
//!
//! Handshake errors include the destination host and port, the TLS validation
//! name, any applied name override, and the connected peer address when the
//! connector exposes it. Custom connectors can attach [`PeerAddr`] to their
//! connection metadata. Request credentials, paths, and queries are not included.
//!
//! Failed rustls certificate checks also include the peer certificate subject,
//! subject alternative names, and SHA-256 fingerprint when the backend was built
//! with `fetch_tls`. These details are unverified. Native TLS failures do not
//! expose the peer certificate or the emitted server name indication. Certificate
//! details are also unavailable for caller-supplied rustls configurations.
//! Unavailable fields include a reason.
//!
//! Diagnostics do not disable certificate checks. The original validation error
//! remains reachable through the error source chain.
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
pub use connection::{Connect, HyperIo, PeerAddr};

// Installs a silent, always-interested global `tracing` subscriber before any
// unit test in this crate runs. This keeps `tracing` emission paths executing
// deterministically (never poisoned into the "disabled" state) and lets per-test
// thread-local subscribers compose safely. See `docs/tracing-tests.md`.
#[cfg(test)]
testing_aids::init_tracing!();
