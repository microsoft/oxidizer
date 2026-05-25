// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_tls/favicon.ico")]

//! Backend-agnostic TLS configuration for HTTP clients.
//!
//! Build a [`TlsOptions`] with [`TlsOptions::builder_rustls`] or
//! [`TlsOptions::builder_native_tls`], or wrap a pre-built
//! [`rustls::ClientConfig`](::rustls::ClientConfig) /
//! [`native_tls::TlsConnector`](::native_tls::TlsConnector) via `From`/`Into`.
//! Materialize a [`TlsBackend`] with [`TlsOptions::build_backend`].
//!
//! # Features
//!
//! - **`rustls`** — pure-Rust [`rustls`](::rustls). `fetch_tls` does not
//!   bundle a crypto provider; the caller supplies one (along with a
//!   server-certificate verifier) via
//!   [`TlsBackendDefaults::configure_rustls`].
//! - **`native-tls`** — platform native TLS (`SChannel` on Windows,
//!   Security Framework on `macOS`, `OpenSSL` on Linux).
//!
//! With neither feature enabled, [`TlsOptions::default`] still constructs but
//! [`TlsOptions::build_backend`] returns a [`BackendError`].

mod backend;
mod options;

pub use backend::{BackendError, TlsBackend, TlsBackendDefaults};
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
pub use rustls::{RustlsOptions, ServerCertVerifierFactory};

#[cfg(test)]
mod testing;
