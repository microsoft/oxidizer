// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds [`TlsBackend`] values from [`TlsOptions`].

use http::Version;

use crate::backend::{BackendError, TlsBackend};
use crate::options::{SharedOptions, TlsOptions, TlsOptionsKind};

/// Builds a [`TlsBackend`] from [`TlsOptions`] and environment defaults.
///
/// HTTP client crates provide platform and policy values, such as the crypto
/// provider, root store, and default backend. Each backend that needs these
/// values has a separate method. Native TLS and pre-configured backends do not
/// use rustls defaults.
///
/// A `TlsBackendBuilder` also contains:
///
/// - The default backend for [`TlsOptions`] that does not select one.
/// - The default HTTP versions when the options builder does not set them.
///
/// Use [`new`](Self::new) when you do not need rustls defaults.
#[derive(Clone, Debug)]
pub struct TlsBackendBuilder {
    #[cfg(any(feature = "rustls", test))]
    pub(crate) rustls: Option<RustlsDefaults>,

    pub(crate) default: DefaultBackend,
    pub(crate) supported_http_versions: Vec<Version>,
}

/// Environment defaults for the rustls backend.
#[cfg(any(feature = "rustls", test))]
#[derive(Clone, Debug)]
pub(crate) struct RustlsDefaults {
    pub(crate) crypto_provider: std::sync::Arc<::rustls::crypto::CryptoProvider>,
    pub(crate) verifier: std::sync::Arc<dyn ::rustls::client::danger::ServerCertVerifier>,
}

impl TlsBackendBuilder {
    /// Creates an empty builder.
    ///
    /// An empty builder can build pre-configured backends. If the `native-tls`
    /// feature is enabled, it can also build native TLS options. Rustls returns
    /// [`BackendError`] unless the options or this builder provide a crypto
    /// provider and server certificate verifier.
    #[must_use]
    // #[gamma::skip(fn_value.default, tag = "equivalent", reason = "Self::default() and inferred Default::default() call the same TlsBackendBuilder implementation")]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the default HTTP versions for options that do not set them.
    ///
    /// # Panics
    ///
    /// Panics if `versions` is empty.
    #[must_use]
    pub fn supported_http_versions(mut self, versions: &[Version]) -> Self {
        assert!(
            !versions.is_empty(),
            "supported_http_versions cannot be empty. Configure at least one HTTP version, such as HTTP/1.1 or HTTP/2"
        );
        self.supported_http_versions = versions.to_vec();
        self
    }

    /// Sets the rustls crypto provider and default server certificate verifier.
    ///
    /// The builder uses this verifier when the application does not provide one.
    ///
    /// If no default backend is selected, this method selects rustls. Call
    /// `defaults_to_native_tls` afterward to select native TLS instead.
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
    /// This default applies when [`TlsOptions`] does not select a backend.
    #[cfg(any(feature = "native-tls", test))]
    #[must_use]
    pub fn defaults_to_native_tls(mut self) -> Self {
        self.default = DefaultBackend::NativeTls;
        self
    }

    /// Sets the default backend to rustls.
    ///
    /// This default applies when [`TlsOptions`] does not select a backend.
    /// You must also call [`configure_rustls`](Self::configure_rustls).
    /// Otherwise, [`build_backend`](Self::build_backend) returns
    /// [`BackendError`].
    #[cfg(any(feature = "rustls", test))]
    #[must_use]
    pub fn defaults_to_rustls(mut self) -> Self {
        self.default = DefaultBackend::Rustls;
        self
    }

    /// Builds a [`TlsBackend`] from `options`.
    ///
    /// The result depends on how the caller constructed `options`:
    ///
    /// - Default options use the backend selected on this builder.
    /// - Rustls options use their values before this builder's rustls defaults.
    /// - Native TLS options do not use this builder's backend defaults.
    /// - Pre-configured options return the wrapped backend without changes.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] when no backend is selected, required rustls
    /// defaults are missing, or the backend cannot use the provided values.
    ///
    /// # Panics
    ///
    /// Panics on Apple platforms other than macOS when native TLS options
    /// contain a client identity. `native_tls` does not support client
    /// identities on those platforms.
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
                let (connector, server_name_overrides) = native_backend.build(self, &options.shared)?;
                Ok(TlsBackend::NativeTls(crate::NativeTlsBackend::new(
                    connector,
                    server_name_overrides,
                )))
            }
        }
    }

    #[allow(
        clippy::allow_attributes,
        clippy::no_effect_underscore_binding,
        reason = "feature-gated arms use `shared`. Only Unselected is available when both backend features are disabled"
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
                let (connector, server_name_overrides) = crate::native_tls::NativeTlsOptions::new().build(self, &shared)?;
                Ok(TlsBackend::NativeTls(crate::NativeTlsBackend::new(
                    connector,
                    server_name_overrides,
                )))
            }
            DefaultBackend::Unselected => {
                // Keep shared options in builds where backend features are disabled.
                let _shared = shared;

                Err(BackendError::caused_by(
                    "TlsBackendBuilder has no default TLS backend. Enable a backend feature if needed, then select that backend as the default before building automatic TlsOptions",
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

/// The default TLS backend when [`TlsOptions`] does not select one.
#[derive(Debug, Clone, Default)]
pub(crate) enum DefaultBackend {
    /// No default backend is selected.
    ///
    /// Building automatic [`TlsOptions`] returns [`BackendError`].
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

            #[test]
            #[cfg_attr(miri, ignore)]
            fn native_tls_preserves_server_name_overrides() {
                let tls = TlsOptions::builder_native_tls()
                    .server_name_override("localhost:8443", "expected.example.test")
                    .build();
                let backend = TlsBackendBuilder::new().build_backend(tls).unwrap();
                let TlsBackend::NativeTls(backend) = backend else {
                    panic!("expected native TLS backend");
                };

                assert_eq!(
                    backend.server_name_for(&"localhost:8443".parse().unwrap()),
                    Some("expected.example.test")
                );
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
