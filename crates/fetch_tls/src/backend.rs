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
    /// on macOS, `OpenSSL` on Linux).
    #[cfg(any(feature = "native-tls", test))]
    NativeTls(::native_tls::TlsConnector),
}

/// Environment-supplied defaults for materializing a [`TlsBackend`].
///
/// Lets HTTP client crates own platform / policy choices (such as which
/// crypto provider or root store to use) without baking them into
/// `fetch_tls`. Each backend that needs environment state has its own
/// constructor; native-tls and pre-configured backends ignore defaults.
///
/// Use [`TlsBackendDefaults::new`] when no backend-specific state is
/// required. Building a rustls backend without
/// [`TlsBackendDefaults::rustls`] returns a [`BackendError`].
#[derive(Clone, Default)]
pub struct TlsBackendDefaults {
    #[cfg(any(feature = "rustls", test))]
    pub(crate) rustls: Option<RustlsDefaults>,
}

/// Environment-supplied defaults specific to the rustls backend.
#[cfg(any(feature = "rustls", test))]
#[derive(Clone)]
pub(crate) struct RustlsDefaults {
    pub(crate) crypto_provider: std::sync::Arc<::rustls::crypto::CryptoProvider>,
    pub(crate) verifier: std::sync::Arc<dyn ::rustls::client::danger::ServerCertVerifier>,
}

impl TlsBackendDefaults {
    /// Creates an empty set of defaults.
    ///
    /// Sufficient for native-tls or pre-configured backends; materializing
    /// a rustls backend with these returns a [`BackendError`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Supplies the rustls crypto provider and a fallback server certificate verifier.
    ///
    /// The verifier is used only when the caller did not configure one via
    /// [`TlsOptionsBuilder::server_certificate_verifier`](super::TlsOptionsBuilder::server_certificate_verifier).
    #[cfg(any(feature = "rustls", test))]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls")))]
    #[must_use]
    pub fn rustls(
        crypto_provider: std::sync::Arc<::rustls::crypto::CryptoProvider>,
        verifier: std::sync::Arc<dyn ::rustls::client::danger::ServerCertVerifier>,
    ) -> Self {
        Self {
            rustls: Some(RustlsDefaults { crypto_provider, verifier }),
        }
    }
}

impl std::fmt::Debug for TlsBackendDefaults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("TlsBackendDefaults");
        #[cfg(any(feature = "rustls", test))]
        {
            s.field(
                "rustls",
                &self.rustls.as_ref().map(|_| "<rustls CryptoProvider + ServerCertVerifier>"),
            );
        }
        s.finish()
    }
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
