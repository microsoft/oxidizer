// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TlsOptions`] and its type-state builder [`TlsOptionsBuilder`].

use http::Version;

use crate::backend::{BackendError, DefaultBackend};
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
    /// Backend is selected automatically based on how the default backend
    /// is configured via [`TlsBackendDefaults`]; see its documentation for details.
    Auto,

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
/// For most callers, [`TlsOptions::new_rustls`] and
/// [`TlsOptions::new_native_tls`] are the simplest way to construct
/// [`TlsOptions`]: they produce a configuration with appropriate defaults
/// for the selected backend. When you need to customize the configuration
/// (for example, to set a client identity, override the server certificate
/// verifier, or change supported HTTP versions), use the corresponding
/// builder constructor [`TlsOptions::builder_rustls`] or
/// [`TlsOptions::builder_native_tls`] instead.
///
/// If you do not want to pick a backend yourself, use
/// [`TlsOptions::default`] to defer that choice to the HTTP client that
/// adopts `fetch_tls`.
///
/// You can also convert from a pre-built
/// [`rustls::ClientConfig`](::rustls::ClientConfig) /
/// [`native_tls::TlsConnector`](::native_tls::TlsConnector) via
/// [`From`]/[`Into`] to wrap a backend you have already built.
///
/// # Examples
///
/// Minimal rustls-backed [`TlsOptions`] using default settings; supply a
/// [`TlsBackendDefaults::configure_rustls`](crate::TlsBackendDefaults::configure_rustls)
/// when calling [`TlsOptions::build_backend`] to materialize a backend:
///
/// ```rust,no_run
/// # #[cfg(feature = "rustls")] {
/// use fetch_tls::TlsOptions;
///
/// let tls = TlsOptions::new_rustls();
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
/// let tls = TlsOptions::new_native_tls();
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

    /// Materializes these options into a [`TlsBackend`] using `defaults`.
    ///
    /// - auto — selected by [`TlsOptions::default`]; the backend is chosen
    ///   from [`TlsBackendDefaults`]'s configured default (set via
    ///   [`TlsBackendDefaults::defaults_to_rustls`] /
    ///   [`TlsBackendDefaults::defaults_to_native_tls`], or implicitly by
    ///   [`TlsBackendDefaults::configure_rustls`]).
    /// - rustls — requires [`TlsBackendDefaults::configure_rustls`]; values
    ///   configured on the builder take precedence over those in `defaults`.
    /// - native-tls — `defaults` is ignored.
    /// - pre-configured — the wrapped backend is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if no backend is selected, if required
    /// rustls defaults are missing, or if backend construction fails (for
    /// example, invalid client identity material).
    pub fn build_backend(self, defaults: &TlsBackendDefaults) -> Result<TlsBackend, BackendError> {
        match self.inner {
            TlsOptionsKind::Auto => self.build_auto_backend(defaults),
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

    #[allow(
        clippy::allow_attributes,
        clippy::unused_self,
        reason = "self.shared is used by feature-gated arms; with neither rustls nor native-tls enabled only Unselected is reachable"
    )]
    fn build_auto_backend(self, defaults: &TlsBackendDefaults) -> Result<TlsBackend, BackendError> {
        match defaults.default {
            #[cfg(any(feature = "rustls", test))]
            DefaultBackend::Rustls => {
                let config = super::rustls::RustlsOptions::new().build(defaults.rustls.as_ref(), &self.shared)?;
                Ok(TlsBackend::Rustls(std::sync::Arc::new(config)))
            }
            #[cfg(any(feature = "native-tls", test))]
            DefaultBackend::NativeTls => {
                let connector = super::native_tls::NativeTlsOptions::new().build(&self.shared)?;
                Ok(TlsBackend::NativeTls(connector))
            }
            DefaultBackend::Unselected => Err(BackendError::caused_by(
                "no default TLS backend is configured on TlsBackendDefaults; call defaults_to_rustls() / defaults_to_native_tls() (or configure_rustls(), which implies rustls), or construct TlsOptions via one of its builders",
            )),
        }
    }
}

/// Constructs [`TlsOptions`] whose backend is chosen at
/// [`TlsOptions::build_backend`] time from the supplied
/// [`TlsBackendDefaults`]. See [`TlsBackendDefaults`] for how to select the
/// default backend.
impl Default for TlsOptions {
    fn default() -> Self {
        Self {
            inner: TlsOptionsKind::Auto,
            shared: SharedOptions::default(),
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
    ///
    /// # Panics
    ///
    /// Panics if `versions` is empty.
    pub fn supported_http_versions(mut self, versions: &[Version]) -> Self {
        assert!(
            !versions.is_empty(),
            "supported_http_versions cannot be empty; configure at least one HTTP version (for example HTTP/1.1 or HTTP/2)"
        );
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
    use std::sync::Arc;

    use rustls::crypto::aws_lc_rs;

    use super::*;
    use crate::testing::AcceptAllServerCertVerifier as AcceptAll;

    #[test]
    fn default_supported_http_versions_is_http1_and_http2() {
        let shared = SharedOptions::default();
        assert_eq!(shared.supported_http_versions, vec![Version::HTTP_11, Version::HTTP_2]);
    }

    #[test]
    fn default_constructs_auto_variant() {
        let tls = TlsOptions::default();
        assert!(matches!(tls.inner, TlsOptionsKind::Auto));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn auto_without_default_backend_returns_error() {
        let defaults = TlsBackendDefaults::new();

        let err = TlsOptions::default().build_backend(&defaults).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no default TLS backend"), "unexpected error: {msg}");
    }

    #[test]
    fn supported_http_versions_round_trips() {
        let tls = TlsOptions::builder_rustls()
            .supported_http_versions(&[Version::HTTP_11, Version::HTTP_2])
            .build();
        assert_eq!(tls.supported_http_versions(), &[Version::HTTP_11, Version::HTTP_2]);
    }

    fn rustls_defaults() -> TlsBackendDefaults {
        TlsBackendDefaults::new().configure_rustls(Arc::new(aws_lc_rs::default_provider()), Arc::new(AcceptAll))
    }

    mod build_backend_rustls {
        use super::*;

        #[test]
        #[cfg_attr(miri, ignore)]
        fn rustls_falls_back_to_default_verifier() {
            let tls = TlsOptions::builder_rustls().build();
            let backend = tls.build_backend(&rustls_defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn rustls_uses_caller_verifier_when_set() {
            let tls = TlsOptions::builder_rustls()
                .server_certificate_verifier(|_| Arc::new(AcceptAll))
                .build();
            let backend = tls.build_backend(&rustls_defaults()).unwrap();
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
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAll))
                .with_no_client_auth();
            let tls = TlsOptions::from(config);
            let backend = tls.build_backend(&rustls_defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }
    }

    mod build_backend_native_tls {
        use super::*;

        #[test]
        #[cfg_attr(miri, ignore)]
        fn native_tls_ignores_rustls_defaults() {
            let tls = TlsOptions::builder_native_tls().build();
            let backend = tls.build_backend(&TlsBackendDefaults::new()).unwrap();
            assert!(matches!(backend, TlsBackend::NativeTls(_)));
        }
    }

    mod build_backend_auto {
        use super::*;

        #[test]
        #[cfg_attr(miri, ignore)]
        fn configure_rustls_auto_promotes_unselected_to_rustls() {
            let backend = TlsOptions::default().build_backend(&rustls_defaults()).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn defaults_to_rustls_selects_rustls() {
            let defaults = rustls_defaults().defaults_to_rustls();
            let backend = TlsOptions::default().build_backend(&defaults).unwrap();
            assert!(matches!(backend, TlsBackend::Rustls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn defaults_to_rustls_without_rustls_defaults_returns_crypto_provider_error() {
            let defaults = TlsBackendDefaults::new().defaults_to_rustls();
            let err = TlsOptions::default().build_backend(&defaults).unwrap_err();
            let msg = format!("{err}");
            assert!(msg.contains("crypto provider"), "unexpected error: {msg}");
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn defaults_to_native_tls_selects_native_tls() {
            let defaults = TlsBackendDefaults::new().defaults_to_native_tls();
            let backend = TlsOptions::default().build_backend(&defaults).unwrap();
            assert!(matches!(backend, TlsBackend::NativeTls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn defaults_to_native_tls_after_configure_rustls_overrides_promotion() {
            let defaults = rustls_defaults().defaults_to_native_tls();
            let backend = TlsOptions::default().build_backend(&defaults).unwrap();
            assert!(matches!(backend, TlsBackend::NativeTls(_)));
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn configure_rustls_after_defaults_to_native_tls_keeps_native_tls() {
            let defaults = TlsBackendDefaults::new()
                .defaults_to_native_tls()
                .configure_rustls(Arc::new(aws_lc_rs::default_provider()), Arc::new(AcceptAll));
            let backend = TlsOptions::default().build_backend(&defaults).unwrap();
            assert!(matches!(backend, TlsBackend::NativeTls(_)));
        }
    }
}
