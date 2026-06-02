// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/favicon.ico")]

//! Backend-agnostic TLS configuration for HTTP clients.
//!
//! `fetch_tls` separates *what* TLS behavior a consumer wants from *which*
//! TLS implementation actually provides it. This lets application code
//! describe its TLS requirements once and lets the HTTP client (or other
//! library) decide which backend to materialize at runtime.
//!
//! # Overview
//!
//! The core type is [`TlsOptions`], a backend-agnostic description of a TLS
//! client configuration. It can be constructed in a few ways:
//!
//! - **Explicit backend selection** — use [`TlsOptions::new_rustls`] or
//!   [`TlsOptions::new_native_tls`] when the consumer specifically wants
//!   `rustls` or `native-tls`. For additional customization (client
//!   identity, custom verifier, supported HTTP versions, etc.), use the
//!   corresponding builders [`TlsOptions::builder_rustls`] /
//!   [`TlsOptions::builder_native_tls`] (which return a
//!   [`TlsOptionsBuilder`]).
//! - **Wrapping a pre-built configuration** — convert an existing
//!   [`rustls::ClientConfig`](::rustls::ClientConfig) or
//!   [`native_tls::TlsConnector`](::native_tls::TlsConnector) into a
//!   [`TlsOptions`] via `From`/`Into`.
//! - **Default construction** — [`TlsOptions::default`] produces options
//!   that do not pin a specific backend; the choice is deferred to the
//!   library consuming the [`TlsOptions`].
//!
//! # User vs. consumer perspective
//!
//! From an **end-user / application** perspective, only [`TlsOptions`] and
//! [`TlsOptionsBuilder`] are relevant. Users describe their TLS requirements
//! and hand the resulting [`TlsOptions`] to a library (for example, an HTTP
//! client).
//!
//! From a **library / client** perspective that adopts `fetch_tls`, the
//! [`TlsOptions`] is materialized into a concrete [`TlsBackend`] (backed by
//! a specific TLS implementation) via [`TlsBackendBuilder::build_backend`].
//! The library supplies a [`TlsBackendBuilder`] that:
//!
//! - provides the information required to actually build the backend (for
//!   example, a `rustls` crypto provider and server-certificate verifier via
//!   [`TlsBackendBuilder::configure_rustls`]), and
//! - decides which backend to use when the [`TlsOptions`] does not pin one
//!   (i.e. when the consumer does not care about the underlying TLS
//!   technology).
//!
//! # Features
//!
//! - **`rustls`** — pure-Rust [`rustls`](::rustls). `fetch_tls` does not
//!   bundle a crypto provider; the adopting library supplies one (along
//!   with a server-certificate verifier) via
//!   [`TlsBackendBuilder::configure_rustls`].
//! - **`native-tls`** — platform native TLS (`SChannel` on Windows,
//!   Security Framework on `macOS`, `OpenSSL` on Linux).
//!
//! With neither feature enabled, [`TlsOptions::default`] still constructs
//! but [`TlsBackendBuilder::build_backend`] returns a [`BackendError`].

#[cfg(any(feature = "native-tls", feature = "rustls", test))]
mod alpn;
mod backend;
mod backend_builder;
mod options;

pub use backend::{BackendError, TlsBackend};
pub use backend_builder::TlsBackendBuilder;
pub use options::{TlsOptions, TlsOptionsBuilder};

mod client_identity;
pub use client_identity::{ClientIdentity, ClientIdentityError};

#[cfg(any(feature = "native-tls", test))]
mod native_tls;
#[cfg(any(feature = "native-tls", test))]
pub use native_tls::NativeTlsOptions;

#[cfg(any(feature = "rustls", test))]
mod rustls;
#[cfg(any(feature = "rustls", test))]
pub use rustls::RustlsOptions;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod testing;
