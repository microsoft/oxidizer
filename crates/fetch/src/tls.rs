// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! TLS configuration for HTTP clients.
//!
//! [`fetch_tls`] defines the TLS types. This module re-exports them. The `rustls` and `native-tls`
//! Cargo features select the available backends.

#[cfg(any(feature = "native-tls", test))]
pub use fetch_tls::NativeTlsOptions;
#[cfg(any(feature = "rustls", test))]
pub use fetch_tls::RustlsOptions;
pub use fetch_tls::{AutoBackend, ClientIdentity, ClientIdentityError, TlsOptions, TlsOptionsBuilder};
