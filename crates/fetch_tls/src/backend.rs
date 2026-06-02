// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fully constructed TLS backends ready for use by an HTTP client.

/// Error returned when materializing a [`TlsBackend`] from
/// [`TlsOptions`](super::TlsOptions) fails.
#[ohno::error]
pub struct BackendError;

/// A fully constructed TLS backend ready for use by an HTTP client.
///
/// Unlike [`TlsOptions`](super::TlsOptions), which describes *how* to build
/// a TLS configuration, `TlsBackend` holds the resulting backend-specific
/// state. Which variants are available depends on enabled features:
/// [`TlsBackend::Rustls`] requires `rustls`; [`TlsBackend::NativeTls`]
/// requires `native-tls`.
///
/// Typically produced from [`TlsOptions`](super::TlsOptions); construct
/// directly only when wrapping a pre-built backend.
#[derive(Debug, Clone)]
#[allow(
    clippy::allow_attributes,
    clippy::large_enum_variant,
    reason = "configuration object; boxing would clutter the public API without performance benefit"
)]
pub enum TlsBackend {
    /// rustls backend, carrying a shared [`ClientConfig`](::rustls::ClientConfig).
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
