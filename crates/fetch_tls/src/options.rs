// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TlsOptions`] and its type-state builder [`TlsOptionsBuilder`].

use http::Version;

use crate::backend::BackendError;
use crate::client_identity::ClientIdentity;
use crate::{TlsBackend, TlsBackendDefaults};

/// Internal representation of a TLS configuration.
#[derive(Debug, Clone)]
#[allow(
    clippy::allow_attributes,
    clippy::large_enum_variant,
    dead_code,
    reason = "configuration object; variants are consumed by downstream HTTP client crates that materialize TLS backends from TlsOptions"
)]
pub(crate) enum TlsOptionsKind {
    /// No backend selected. Produced by [`TlsOptions::empty`] (and by
    /// [`TlsOptions::default`] when neither the `rustls` nor `native-tls`
    /// feature is enabled). Calling [`TlsOptions::build_backend`] on such
    /// options returns a [`BackendError`].
    Empty,

    /// Pre-configured TLS backend, used as-is without any modifications.
    PreConfigured(TlsBackend),

    /// rustls backend.
    #[cfg(any(feature = "rustls", test))]
    Rustls(super::rustls::RustlsOptions),

    /// Platform native TLS backend.
    #[cfg(any(feature = "native-tls", test))]
    NativeTls(super::native_tls::NativeTlsOptions),
}

/// TLS configuration for an HTTP client.
///
/// Use [`TlsOptions::builder_rustls`] or [`TlsOptions::builder_native_tls`]
/// to configure a backend from scratch, or convert from a pre-built
/// [`rustls::ClientConfig`](::rustls::ClientConfig) /
/// [`native_tls::TlsConnector`](::native_tls::TlsConnector) via
/// [`From`]/[`Into`] to wrap a backend you have already built.
///
/// # Examples
///
/// Minimal rustls-backed [`TlsOptions`] using default settings; supply a
/// [`TlsBackendDefaults::rustls`](crate::TlsBackendDefaults::rustls) when
/// calling [`TlsOptions::build_backend`] to materialize a backend:
///
/// ```rust,no_run
/// # #[cfg(feature = "rustls")] {
/// use fetch_tls::TlsOptions;
///
/// let tls = TlsOptions::builder_rustls().build();
/// # }
/// ```
///
/// Minimal native-tls-backed [`TlsOptions`] using default settings; no
/// backend defaults are required to materialize the backend:
///
/// ```rust,no_run
/// # #[cfg(feature = "native-tls")] {
/// use fetch_tls::TlsOptions;
///
/// let tls = TlsOptions::builder_native_tls().build();
/// # }
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct TlsOptions {
    #[allow(
        clippy::allow_attributes,
        dead_code,
        reason = "consumed by downstream HTTP client crates that materialize a TlsBackend from TlsOptions"
    )]
    pub(crate) inner: TlsOptionsKind,
    pub(crate) shared: SharedOptions,
}

#[derive(Debug, Clone)]
pub(crate) struct SharedOptions {
    pub(crate) supported_http_versions: Vec<Version>,
    pub(crate) client_identity: Option<ClientIdentity>,
}

impl Default for SharedOptions {
    fn default() -> Self {
        Self {
            supported_http_versions: vec![Version::HTTP_11, Version::HTTP_2],
            client_identity: None,
        }
    }
}

impl TlsOptions {
    /// HTTP versions the caller intends to negotiate. Backends use this list
    /// to compute the `ALPN` protocols offered during the TLS handshake.
    #[must_use]
    pub fn supported_http_versions(&self) -> &[Version] {
        &self.shared.supported_http_versions
    }

    /// Creates an empty [`TlsOptions`] with no backend selected.
    ///
    /// [`TlsOptions::build_backend`] returns a [`BackendError`]. Used by
    /// [`TlsOptions::default`] when neither TLS feature is enabled.
    #[cfg(any(test, not(any(feature = "rustls", feature = "native-tls"))))]
    fn empty() -> Self {
        Self {
            inner: TlsOptionsKind::Empty,
            shared: SharedOptions::default(),
        }
    }

    /// Materializes these options into a [`TlsBackend`] using `defaults`.
    ///
    /// - rustls — requires [`TlsBackendDefaults::rustls`]; values configured
    ///   on the builder take precedence over those in `defaults`.
    /// - native-tls — `defaults` is ignored.
    /// - pre-configured — the wrapped backend is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if no backend is selected, if required
    /// rustls defaults are missing, or if backend construction fails (for
    /// example, invalid client identity material).
    pub fn build_backend(self, defaults: &TlsBackendDefaults) -> Result<TlsBackend, BackendError> {
        let _ = defaults;
        match self.inner {
            TlsOptionsKind::Empty => Err(BackendError::caused_by(
                "no TLS backend is configured; enable the `rustls` or `native-tls` feature, or construct TlsOptions via one of its builders",
            )),
            TlsOptionsKind::PreConfigured(backend) => Ok(backend),
            #[cfg(any(feature = "rustls", test))]
            TlsOptionsKind::Rustls(rustls_backend) => {
                let config = rustls_backend.build(defaults.rustls.as_ref(), &self.shared)?;
                Ok(TlsBackend::Rustls(std::sync::Arc::new(config)))
            }
            #[cfg(any(feature = "native-tls", test))]
            TlsOptionsKind::NativeTls(native_backend) => {
                let connector = native_backend.build(&self.shared)?;
                Ok(TlsBackend::NativeTls(connector))
            }
        }
    }
}

/// Picks a default TLS backend from enabled features:
/// `rustls` if available, else `native-tls`, else [`TlsOptions::empty`].
impl Default for TlsOptions {
    fn default() -> Self {
        #[cfg(feature = "rustls")]
        {
            Self::builder_rustls().build()
        }
        #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
        {
            Self::builder_native_tls().build()
        }
        #[cfg(not(any(feature = "native-tls", feature = "rustls")))]
        {
            Self::empty()
        }
    }
}

/// Type-state builder for [`TlsOptions`], parameterized by backend.
///
/// `B` selects the backend: [`RustlsOptions`](super::RustlsOptions) (rustls)
/// or [`NativeTlsOptions`](super::NativeTlsOptions) (platform native).
/// Obtain via [`TlsOptions::builder_rustls`] or
/// [`TlsOptions::builder_native_tls`], then call `.build()`.
#[derive(Debug, Clone)]
#[must_use]
pub struct TlsOptionsBuilder<B> {
    #[allow(
        clippy::allow_attributes,
        dead_code,
        reason = "Read by feature-gated backend impls (rustls/native-tls); unused when neither feature is enabled."
    )]
    pub(crate) backend: B,
    pub(crate) shared: SharedOptions,
}

impl<B> TlsOptionsBuilder<B> {
    /// Sets the HTTP versions the client intends to negotiate.
    ///
    /// Backends derive the advertised `ALPN` protocols from this list.
    pub fn supported_http_versions(mut self, versions: &[Version]) -> Self {
        self.shared.supported_http_versions = versions.to_vec();
        self
    }

    /// Sets the client identity for mutual TLS (`mTLS`) authentication.
    ///
    /// The same identity works for either backend; backend-specific
    /// conversion happens in [`TlsOptions::build_backend`]. The native-tls
    /// backend requires the private key to be `PKCS#8`.
    pub fn client_identity(mut self, identity: ClientIdentity) -> Self {
        self.shared.client_identity = Some(identity);
        self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_supported_http_versions_is_http1_and_http2() {
        let shared = SharedOptions::default();
        assert_eq!(shared.supported_http_versions, vec![Version::HTTP_11, Version::HTTP_2]);
    }

    #[test]
    fn empty_constructs_empty_variant() {
        let tls = TlsOptions::empty();
        assert!(matches!(tls.inner, TlsOptionsKind::Empty));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn empty_build_backend_returns_error() {
        let defaults = TlsBackendDefaults::new();

        let err = TlsOptions::empty().build_backend(&defaults).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no TLS backend"), "unexpected error: {msg}");
    }

    #[cfg(feature = "rustls")]
    #[test]
    fn default_selects_rustls_when_feature_enabled() {
        let tls = TlsOptions::default();
        assert!(matches!(tls.inner, TlsOptionsKind::Rustls(_)));
    }

    #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
    #[test]
    fn default_selects_native_tls_when_only_native_tls_enabled() {
        let tls = TlsOptions::default();
        assert!(matches!(tls.inner, TlsOptionsKind::NativeTls(_)));
    }

    #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
    #[test]
    fn default_is_empty_when_no_features_enabled() {
        let tls = TlsOptions::default();
        assert!(matches!(tls.inner, TlsOptionsKind::Empty));
    }

    #[cfg(feature = "rustls")]
    #[test]
    fn supported_http_versions_round_trips() {
        let tls = TlsOptions::builder_rustls()
            .supported_http_versions(&[Version::HTTP_11, Version::HTTP_2])
            .build();
        assert_eq!(tls.supported_http_versions(), &[Version::HTTP_11, Version::HTTP_2]);
    }

    #[cfg(feature = "rustls")]
    mod build_backend_rustls {
        use std::sync::Arc;

        use super::*;
        use crate::testing::AcceptAllServerCertVerifier as AcceptAll;

        fn defaults() -> TlsBackendDefaults {
            TlsBackendDefaults::rustls(Arc::new(rustls_symcrypt::default_symcrypt_provider()), Arc::new(AcceptAll))
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn rustls_falls_back_to_default_verifier() {
            let tls = TlsOptions::builder_rustls().build();
            let backend = tls.build_backend(&defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn rustls_uses_caller_verifier_when_set() {
            let tls = TlsOptions::builder_rustls()
                .server_certificate_verifier(|_| Arc::new(AcceptAll))
                .build();
            let backend = tls.build_backend(&defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn rustls_without_defaults_returns_error() {
            let tls = TlsOptions::builder_rustls().build();
            let err = tls.build_backend(&TlsBackendDefaults::new()).unwrap_err();
            let msg = format!("{err}");
            assert!(msg.contains("crypto provider"), "unexpected error: {msg}");
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn preconfigured_passes_backend_through_unchanged() {
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls_symcrypt::default_symcrypt_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAll))
                .with_no_client_auth();
            let tls = TlsOptions::from(config);
            let backend = tls.build_backend(&defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }
    }

    #[cfg(feature = "native-tls")]
    mod build_backend_native_tls {
        use super::*;

        fn defaults() -> TlsBackendDefaults {
            TlsBackendDefaults::new()
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn native_tls_ignores_rustls_defaults() {
            let tls = TlsOptions::builder_native_tls().build();
            let backend = tls.build_backend(&defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::NativeTls(_)));
        }
    }
}
