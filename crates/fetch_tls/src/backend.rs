// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fully constructed TLS backends ready for use by an HTTP client.

/// Error returned when materializing a [`TlsBackend`] from
/// [`TlsOptions`](super::TlsOptions) fails.
#[ohno::error]
pub struct BackendError;

/// A fully constructed TLS backend ready for use by an HTTP client.
///
/// Where [`TlsOptions`](super::TlsOptions) describes *how* to build a TLS
/// configuration, `TlsBackend` holds the result. Which variants are
/// available depends on the enabled Cargo features.
///
/// Typically produced by [`TlsBackendBuilder`](super::TlsBackendBuilder);
/// construct directly only when wrapping a backend you have already built.
#[derive(Debug, Clone)]
#[allow(
    clippy::allow_attributes,
    clippy::large_enum_variant,
    reason = "configuration object; boxing would clutter the public API without performance benefit"
)]
pub enum TlsBackend {
    /// rustls backend, carrying a shared `rustls::ClientConfig`.
    #[cfg(any(feature = "rustls", test))]
    Rustls(std::sync::Arc<::rustls::ClientConfig>),

    /// Platform native TLS backend (`SChannel` on Windows, Security Framework
    /// on `macOS`, `OpenSSL` on Linux).
    #[cfg(any(feature = "native-tls", test))]
    NativeTls(::native_tls::TlsConnector),
}

#[cfg(any(feature = "rustls", test))]
impl From<::rustls::ClientConfig> for TlsBackend {
    fn from(config: ::rustls::ClientConfig) -> Self {
        Self::Rustls(std::sync::Arc::new(config))
    }
}

#[cfg(any(feature = "rustls", test))]
impl From<std::sync::Arc<::rustls::ClientConfig>> for TlsBackend {
    fn from(config: std::sync::Arc<::rustls::ClientConfig>) -> Self {
        Self::Rustls(config)
    }
}

#[cfg(any(feature = "native-tls", test))]
impl From<::native_tls::TlsConnector> for TlsBackend {
    fn from(connector: ::native_tls::TlsConnector) -> Self {
        Self::NativeTls(connector)
    }
}
