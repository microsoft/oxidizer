// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TlsBackendBuilder`] — materializes [`TlsOptions`] into a [`TlsBackend`].

use http::Version;

use crate::backend::{BackendError, TlsBackend};
use crate::options::{SharedOptions, TlsOptions, TlsOptionsKind};

/// Builds a [`TlsBackend`] from a [`TlsOptions`] using environment-supplied
/// defaults.
///
/// Lets HTTP client crates own platform and policy choices (such as which
/// crypto provider, root store, or default backend to use) without baking
/// them into `fetch_tls`. Each backend that needs environment state has its
/// own setter; the native-tls and pre-configured backends do not consult
/// these defaults.
///
/// In addition to backend-specific defaults, a `TlsBackendBuilder` carries:
///
/// - the default backend used when a [`TlsOptions`] does not pin one, and
/// - the default list of supported HTTP versions used when the options
///   builder did not set them.
///
/// Use [`new`](Self::new) when no backend-specific state is required.
#[derive(Clone, Debug)]
pub struct TlsBackendBuilder {
    #[cfg(any(feature = "rustls", test))]
    pub(crate) rustls: Option<RustlsDefaults>,

    pub(crate) default: DefaultBackend,
    pub(crate) supported_http_versions: Vec<Version>,
}

/// Environment-supplied defaults specific to the rustls backend.
#[cfg(any(feature = "rustls", test))]
#[derive(Clone, Debug)]
pub(crate) struct RustlsDefaults {
    pub(crate) crypto_provider: std::sync::Arc<::rustls::crypto::CryptoProvider>,
    pub(crate) verifier: std::sync::Arc<dyn ::rustls::client::danger::ServerCertVerifier>,
}

impl TlsBackendBuilder {
    /// Creates an empty builder.
    ///
    /// Sufficient for the native-tls and pre-configured backends.
    /// Materializing a rustls backend from an empty builder returns a
    /// [`BackendError`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the default HTTP versions used when the options builder did not
    /// set them.
    ///
    /// # Panics
    ///
    /// Panics if `versions` is empty.
    #[must_use]
    pub fn supported_http_versions(mut self, versions: &[Version]) -> Self {
        assert!(
            !versions.is_empty(),
            "supported_http_versions cannot be empty; configure at least one HTTP version (for example HTTP/1.1 or HTTP/2)"
        );
        self.supported_http_versions = versions.to_vec();
        self
    }

    /// Configures the rustls crypto provider and a fallback server
    /// certificate verifier.
    ///
    /// The verifier is used only when the application did not supply one of
    /// its own on the options builder.
    ///
    /// If no default backend has been selected yet, this call also promotes
    /// rustls to be the default backend. Call `defaults_to_native_tls`
    /// afterwards to override that promotion.
    #[cfg(any(feature = "rustls", test))]
    #[must_use]
    pub fn configure_rustls(
        mut self,
        crypto_provider: std::sync::Arc<::rustls::crypto::CryptoProvider>,
        verifier: std::sync::Arc<dyn ::rustls::client::danger::ServerCertVerifier>,
    ) -> Self {
        self.rustls = Some(RustlsDefaults { crypto_provider, verifier });

        if matches!(self.default, DefaultBackend::Unselected) {
            self.default = DefaultBackend::Rustls;
        }

        self
    }

    /// Sets the default backend to native-tls.
    ///
    /// This default applies to any [`TlsOptions`] that did not pin a
    /// backend itself.
    #[cfg(any(feature = "native-tls", test))]
    #[must_use]
    pub fn defaults_to_native_tls(mut self) -> Self {
        self.default = DefaultBackend::NativeTls;
        self
    }

    /// Sets the default backend to rustls.
    ///
    /// This default applies to any [`TlsOptions`] that did not pin a
    /// backend itself. rustls still requires `configure_rustls` to be
    /// called; selecting rustls without configuring it makes
    /// [`build_backend`](Self::build_backend) fail with a [`BackendError`].
    #[cfg(any(feature = "rustls", test))]
    #[must_use]
    pub fn defaults_to_rustls(mut self) -> Self {
        self.default = DefaultBackend::Rustls;
        self
    }

    /// Materializes `options` into a [`TlsBackend`] using this builder.
    ///
    /// Behavior depends on how `options` was constructed:
    ///
    /// - default (no backend pinned) — uses this builder's configured
    ///   default backend.
    /// - rustls — requires `configure_rustls` to have been called;
    ///   values set on the options builder take precedence over the
    ///   defaults on this builder.
    /// - native-tls — this builder is ignored.
    /// - pre-configured — the wrapped backend is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] if no backend is selected, if required
    /// rustls defaults are missing, or if backend construction fails (for
    /// example, invalid client identity material).
    pub fn build_backend(&self, options: TlsOptions) -> Result<TlsBackend, BackendError> {
        match options.inner {
            TlsOptionsKind::Auto => self.build_auto_backend(options.shared),
            TlsOptionsKind::PreConfigured(backend) => Ok(backend),
            #[cfg(any(feature = "rustls", test))]
            TlsOptionsKind::Rustls(rustls_backend) => {
                let config = rustls_backend.build(self, &options.shared)?;
                Ok(TlsBackend::Rustls(std::sync::Arc::new(config)))
            }
            #[cfg(any(feature = "native-tls", test))]
            TlsOptionsKind::NativeTls(native_backend) => {
                let connector = native_backend.build(self, &options.shared)?;
                Ok(TlsBackend::NativeTls(connector))
            }
        }
    }

    #[allow(
        clippy::allow_attributes,
        clippy::no_effect_underscore_binding,
        reason = "`shared` is used by feature-gated arms; with neither rustls nor native-tls enabled only Unselected is reachable"
    )]
    fn build_auto_backend(&self, shared: SharedOptions) -> Result<TlsBackend, BackendError> {
        match self.default {
            #[cfg(any(feature = "rustls", test))]
            DefaultBackend::Rustls => {
                let config = crate::rustls::RustlsOptions::new().build(self, &shared)?;
                Ok(TlsBackend::Rustls(std::sync::Arc::new(config)))
            }
            #[cfg(any(feature = "native-tls", test))]
            DefaultBackend::NativeTls => {
                let connector = crate::native_tls::NativeTlsOptions::new().build(self, &shared)?;
                Ok(TlsBackend::NativeTls(connector))
            }
            DefaultBackend::Unselected => {
                // use the shared options
                let _shared = shared;

                Err(BackendError::caused_by(
                    "no default TLS backend is configured on TlsBackendBuilder; call defaults_to_rustls() / defaults_to_native_tls() (or configure_rustls(), which implies rustls), or construct TlsOptions via one of its builders",
                ))
            }
        }
    }
}

impl Default for TlsBackendBuilder {
    fn default() -> Self {
        Self {
            #[cfg(any(feature = "rustls", test))]
            rustls: None,
            default: DefaultBackend::Unselected,
            supported_http_versions: vec![Version::HTTP_11, Version::HTTP_2],
        }
    }
}

/// Default TLS backend used when a [`TlsOptions`] does not pin one.
#[derive(Debug, Clone, Default)]
pub(crate) enum DefaultBackend {
    /// No default backend chosen. Building an unpinned [`TlsOptions`]
    /// against such a builder returns a [`BackendError`].
    #[default]
    Unselected,

    #[cfg(any(feature = "rustls", test))]
    Rustls,

    #[cfg(any(feature = "native-tls", test))]
    NativeTls,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_supported_http_versions_is_http1_and_http2() {
        let builder = TlsBackendBuilder::new();
        assert_eq!(builder.supported_http_versions, vec![Version::HTTP_11, Version::HTTP_2]);
    }

    #[test]
    fn supported_http_versions_overrides_defaults() {
        let builder = TlsBackendBuilder::new().supported_http_versions(&[Version::HTTP_11]);
        assert_eq!(builder.supported_http_versions, vec![Version::HTTP_11]);
    }

    #[test]
    fn tls_backend_builder_is_cloneable() {
        static_assertions::assert_impl_all!(TlsBackendBuilder: Clone);
    }

    mod build_backend {
        use std::sync::Arc;

        use ::rustls::crypto::aws_lc_rs;

        use super::*;
        use crate::testing::AcceptAllServerCertVerifier as AcceptAll;

        fn rustls_defaults() -> TlsBackendBuilder {
            TlsBackendBuilder::new().configure_rustls(Arc::new(aws_lc_rs::default_provider()), Arc::new(AcceptAll))
        }

        #[test]
        #[cfg_attr(miri, ignore)]
        fn auto_without_default_backend_returns_error() {
            let defaults = TlsBackendBuilder::new();

            let err = defaults.build_backend(TlsOptions::default()).unwrap_err();
            let msg = format!("{err}");
            assert!(msg.contains("no default TLS backend"), "unexpected error: {msg}");
        }

        mod rustls {
            use super::*;

            #[test]
            #[cfg_attr(miri, ignore)]
            fn rustls_falls_back_to_default_verifier() {
                let tls = TlsOptions::builder_rustls().build();
                let backend = rustls_defaults().build_backend(tls).unwrap();
                assert!(matches!(backend, TlsBackend::Rustls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn rustls_uses_caller_verifier_when_set() {
                let tls = TlsOptions::builder_rustls()
                    .server_certificate_verifier(|_| Arc::new(AcceptAll))
                    .build();
                let backend = rustls_defaults().build_backend(tls).unwrap();
                assert!(matches!(backend, TlsBackend::Rustls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn rustls_without_defaults_returns_error() {
                let tls = TlsOptions::builder_rustls().build();
                let err = TlsBackendBuilder::new().build_backend(tls).unwrap_err();
                let msg = format!("{err}");
                assert!(msg.contains("crypto provider"), "unexpected error: {msg}");
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn preconfigured_passes_backend_through_unchanged() {
                let config = ::rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(AcceptAll))
                    .with_no_client_auth();
                let tls = TlsOptions::from(config);
                let backend = rustls_defaults().build_backend(tls).unwrap();
                assert!(matches!(backend, TlsBackend::Rustls(_)));
            }
        }

        mod native_tls {
            use super::*;

            #[test]
            #[cfg_attr(miri, ignore)]
            fn native_tls_ignores_rustls_defaults() {
                let tls = TlsOptions::builder_native_tls().build();
                let backend = TlsBackendBuilder::new().build_backend(tls).unwrap();
                assert!(matches!(backend, TlsBackend::NativeTls(_)));
            }
        }

        mod auto {
            use super::*;

            #[test]
            #[cfg_attr(miri, ignore)]
            fn builder_routes_through_default_backend() {
                let tls = TlsOptions::builder().build();
                let backend = rustls_defaults().build_backend(tls).unwrap();
                assert!(matches!(backend, TlsBackend::Rustls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn builder_propagates_shared_options_to_chosen_backend() {
                let tls = TlsOptions::builder().supported_http_versions(&[Version::HTTP_2]).build();
                let backend = TlsBackendBuilder::new().defaults_to_native_tls().build_backend(tls).unwrap();
                assert!(matches!(backend, TlsBackend::NativeTls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn configure_rustls_auto_promotes_unselected_to_rustls() {
                let backend = rustls_defaults().build_backend(TlsOptions::default()).unwrap();
                assert!(matches!(backend, TlsBackend::Rustls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn defaults_to_rustls_selects_rustls() {
                let defaults = rustls_defaults().defaults_to_rustls();
                let backend = defaults.build_backend(TlsOptions::default()).unwrap();
                assert!(matches!(backend, TlsBackend::Rustls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn defaults_to_rustls_without_rustls_defaults_returns_crypto_provider_error() {
                let defaults = TlsBackendBuilder::new().defaults_to_rustls();
                let err = defaults.build_backend(TlsOptions::default()).unwrap_err();
                let msg = format!("{err}");
                assert!(msg.contains("crypto provider"), "unexpected error: {msg}");
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn defaults_to_native_tls_selects_native_tls() {
                let defaults = TlsBackendBuilder::new().defaults_to_native_tls();
                let backend = defaults.build_backend(TlsOptions::default()).unwrap();
                assert!(matches!(backend, TlsBackend::NativeTls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn defaults_to_native_tls_after_configure_rustls_overrides_promotion() {
                let defaults = rustls_defaults().defaults_to_native_tls();
                let backend = defaults.build_backend(TlsOptions::default()).unwrap();
                assert!(matches!(backend, TlsBackend::NativeTls(_)));
            }

            #[test]
            #[cfg_attr(miri, ignore)]
            fn configure_rustls_after_defaults_to_native_tls_keeps_native_tls() {
                let defaults = TlsBackendBuilder::new()
                    .defaults_to_native_tls()
                    .configure_rustls(Arc::new(aws_lc_rs::default_provider()), Arc::new(AcceptAll));
                let backend = defaults.build_backend(TlsOptions::default()).unwrap();
                assert!(matches!(backend, TlsBackend::NativeTls(_)));
            }
        }
    }
}
