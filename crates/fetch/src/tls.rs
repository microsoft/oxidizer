// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! TLS configuration for HTTP clients.
//!
//! All TLS types are defined in the [`fetch_tls`] crate and re-exported here so
//! that `fetch` callers can configure TLS without depending on `fetch_tls`
//! directly. The concrete backend is selected by the enabled Cargo features
//! (`rustls` and/or `native-tls`), which `fetch` forwards to `fetch_tls`.

#[cfg(any(feature = "native-tls", test))]
pub use fetch_tls::NativeTlsOptions;
#[cfg(any(feature = "rustls", test))]
pub use fetch_tls::RustlsOptions;
pub use fetch_tls::{AutoBackend, ClientIdentity, ClientIdentityError, TlsOptions, TlsOptionsBuilder};
