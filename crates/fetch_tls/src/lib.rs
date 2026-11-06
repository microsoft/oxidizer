// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/favicon.ico")]

//! TLS configuration for HTTP clients.
//!
//! Applications use `fetch_tls` to describe their TLS requirements.
//! HTTP client libraries use those requirements to build a TLS backend.
//!
//! # Applications
//!
//! Use [`TlsOptions`] or [`TlsOptionsBuilder`] to configure TLS. You can let
//! the client library select a backend, select a backend, or wrap a backend
//! that you already built.
//!
//! # HTTP client libraries
//!
//! Use [`TlsBackendBuilder`] to convert [`TlsOptions`] into [`TlsBackend`].
//! The library provides environment-specific values, such as the rustls
//! crypto provider and default certificate verifier. It also selects the
//! backend when the application does not select one.
//!
//! # Cargo features
//!
//! - `rustls` enables the rustls backend. The application or client library
//!   must provide a crypto provider and server certificate verifier.
//! - `native-tls` enables the platform native TLS backend (`SChannel` on
//!   Windows, Security Framework on `macOS`, `OpenSSL` on Linux).
//!
//! If neither feature is enabled, backend-specific constructors and builders
//! are unavailable. Building automatic options returns [`BackendError`].

#[cfg(any(feature = "native-tls", feature = "rustls", test))]
mod alpn;
mod backend;
mod backend_builder;
mod options;

#[cfg(any(feature = "native-tls", test))]
pub use backend::NativeTlsBackend;
pub use backend::{BackendError, TlsBackend};
pub use backend_builder::TlsBackendBuilder;
pub use options::{AutoBackend, TlsOptions, TlsOptionsBuilder};

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
