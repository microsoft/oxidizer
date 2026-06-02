// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/favicon.ico")]

//! Backend-agnostic TLS configuration for HTTP clients.
//!
//! `fetch_tls` separates *what* TLS behavior an application wants from
//! *which* TLS implementation actually provides it. Applications describe
//! their TLS requirements once, and the HTTP client (or other consuming
//! library) decides which backend to materialize at runtime.
//!
//! # Two perspectives
//!
//! Applications work with [`TlsOptions`] (and its builder,
//! [`TlsOptionsBuilder`]) to describe what they want: pick a specific
//! backend, wrap an already-built backend, or leave the choice to the
//! consuming library.
//!
//! Libraries that adopt `fetch_tls` use [`TlsBackendBuilder`] to turn a
//! [`TlsOptions`] into a ready-to-use [`TlsBackend`]. The library
//! contributes the environment-specific pieces (such as the rustls crypto
//! provider and default certificate verifier) and decides which backend to
//! use when the application did not pin one.
//!
//! # Cargo features
//!
//! - `rustls` — enables the rustls backend. `fetch_tls` does not bundle a
//!   crypto provider; the consuming library supplies one along with a
//!   default server certificate verifier.
//! - `native-tls` — enables the platform native TLS backend (`SChannel` on
//!   Windows, Security Framework on `macOS`, `OpenSSL` on Linux).
//!
//! With neither feature enabled, the API surface is limited to wrapping a
//! pre-built backend; attempting to build any other configuration returns
//! a [`BackendError`].

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
