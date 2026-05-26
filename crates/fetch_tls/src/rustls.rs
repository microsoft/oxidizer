// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rustls backend configuration and builder integration.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::client::ResolvesClientCert;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;

use crate::backend::BackendError;
use crate::options::{SharedOptions, TlsOptions, TlsOptionsBuilder, TlsOptionsKind};

/// Factory that builds a [`ServerCertVerifier`] from the negotiated
/// [`CryptoProvider`].
type ServerCertVerifierFactory = dyn Fn(Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> + Send + Sync;

/// Rustls TLS backend configuration.
#[derive(Clone)]
pub struct RustlsOptions {
    pub(crate) crypto_provider: Option<Arc<CryptoProvider>>,
    pub(crate) verifier_factory: Option<Arc<ServerCertVerifierFactory>>,
    pub(crate) client_identity_resolver: Option<Arc<dyn ResolvesClientCert>>,
}

impl std::fmt::Debug for RustlsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn ServerCertVerifier` and `dyn ResolvesClientCert` do not
        // implement `Debug`; render only presence so we don't have to require
        // Debug on opaque user-supplied types.
        f.debug_struct("RustlsOptions")
            .field("crypto_provider", &self.crypto_provider.as_ref().map(|_| "<custom CryptoProvider>"))
            .field(
                "verifier_factory",
                &self.verifier_factory.as_ref().map(|_| "<custom verifier factory>"),
            )
            .field(
                "client_identity_resolver",
                &self.client_identity_resolver.as_ref().map(|_| "<custom resolver>"),
            )
            .finish()
    }
}

impl RustlsOptions {
    pub(crate) fn new() -> Self {
        Self {
            crypto_provider: None,
            verifier_factory: None,
            client_identity_resolver: None,
        }
    }

    /// Materializes this configuration into a [`rustls::ClientConfig`].
    pub(crate) fn build(
        self,
        defaults: Option<&crate::backend::RustlsDefaults>,
        shared: &SharedOptions,
    ) -> Result<ClientConfig, BackendError> {
        let crypto_provider = self
            .crypto_provider
            .or_else(|| defaults.map(|d| Arc::clone(&d.crypto_provider)))
            .ok_or_else(|| {
                BackendError::caused_by(
                    "rustls crypto provider not supplied; set it via TlsOptionsBuilder::crypto_provider or TlsBackendDefaults::configure_rustls(...)",
                )
            })?;
        let verifier = match self.verifier_factory {
            Some(factory) => factory(Arc::clone(&crypto_provider)),
            None => defaults.map(|d| Arc::clone(&d.verifier)).ok_or_else(|| {
                BackendError::caused_by(
                    "rustls server certificate verifier not supplied; set it via TlsOptionsBuilder::server_certificate_verifier or TlsBackendDefaults::configure_rustls(...)",
                )
            })?,
        };

        let builder = ClientConfig::builder_with_provider(crypto_provider)
            .with_safe_default_protocol_versions()
            .map_err(BackendError::caused_by)?
            .dangerous()
            .with_custom_certificate_verifier(verifier);

        match (self.client_identity_resolver, shared.client_identity.as_ref()) {
            (Some(resolver), _) => Ok(builder.with_client_cert_resolver(resolver)),
            (None, Some(identity)) => builder
                .with_client_auth_cert(identity.cert_chain().to_vec(), identity.private_key().clone_key())
                .map_err(BackendError::caused_by),
            (None, None) => Ok(builder.with_no_client_auth()),
        }
    }
}

impl TlsOptions {
    /// Creates a builder for the rustls backend.
    pub fn builder_rustls() -> TlsOptionsBuilder<RustlsOptions> {
        TlsOptionsBuilder {
            backend: RustlsOptions::new(),
            shared: SharedOptions::default(),
        }
    }

    /// Creates [`TlsOptions`] for the rustls backend using default settings.
    ///
    /// Equivalent to `TlsOptions::builder_rustls().build()`. The crypto
    /// provider and server certificate verifier are taken from the
    /// [`TlsBackendDefaults`](crate::TlsBackendDefaults) passed to
    /// [`TlsOptions::build_backend`]; use [`TlsOptions::builder_rustls`] when
    /// you need to override them or supply a client identity resolver.
    pub fn new_rustls() -> Self {
        Self::builder_rustls().build()
    }
}

/// Wraps a pre-built [`rustls::ClientConfig`] as [`TlsOptions`].
impl From<ClientConfig> for TlsOptions {
    fn from(config: ClientConfig) -> Self {
        Self {
            inner: TlsOptionsKind::PreConfigured(config.into()),
            shared: SharedOptions::default(),
        }
    }
}

/// Wraps a pre-built `Arc<rustls::ClientConfig>` as [`TlsOptions`], avoiding
/// a clone when the config is shared across clients.
impl From<Arc<ClientConfig>> for TlsOptions {
    fn from(config: Arc<ClientConfig>) -> Self {
        Self {
            inner: TlsOptionsKind::PreConfigured(config.into()),
            shared: SharedOptions::default(),
        }
    }
}

impl TlsOptionsBuilder<RustlsOptions> {
    /// Sets the rustls [`CryptoProvider`](rustls::crypto::CryptoProvider).
    ///
    /// Overrides the provider supplied by
    /// [`TlsBackendDefaults::configure_rustls`](crate::TlsBackendDefaults::configure_rustls).
    /// If neither source supplies one, [`TlsOptions::build_backend`] returns
    /// a [`BackendError`](crate::BackendError).
    pub fn crypto_provider(mut self, crypto_provider: Arc<rustls::crypto::CryptoProvider>) -> Self {
        self.backend.crypto_provider = Some(crypto_provider);
        self
    }

    /// Sets a factory that builds the server certificate verifier from the
    /// negotiated [`CryptoProvider`].
    ///
    /// The factory is invoked during [`TlsOptions::build_backend`] with the
    /// provider resolved from this builder or
    /// [`TlsBackendDefaults::configure_rustls`](crate::TlsBackendDefaults::configure_rustls).
    /// Callers that don't need the provider can simply ignore the argument
    /// and return a pre-built verifier (for example, `|_| Arc::new(MyVerifier)`).
    ///
    /// Overrides the verifier supplied by
    /// [`TlsBackendDefaults::configure_rustls`](crate::TlsBackendDefaults::configure_rustls).
    /// If neither source supplies one, [`TlsOptions::build_backend`] returns
    /// a [`BackendError`](crate::BackendError).
    pub fn server_certificate_verifier<F>(mut self, factory: F) -> Self
    where
        F: Fn(Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> + Send + Sync + 'static,
    {
        self.backend.verifier_factory = Some(Arc::new(factory));
        self
    }

    /// Sets a [`ResolvesClientCert`] for mutual TLS authentication.
    ///
    /// Use this when the private key lives behind an external signing oracle
    /// (`HSM`, secure enclave, etc.) instead of in memory. Takes precedence
    /// over [`TlsOptionsBuilder::client_identity`](crate::TlsOptionsBuilder::client_identity).
    pub fn client_identity_resolver(mut self, resolver: Arc<dyn ResolvesClientCert>) -> Self {
        self.backend.client_identity_resolver = Some(resolver);
        self
    }

    /// Builds the final [`TlsOptions`] with the rustls backend.
    pub fn build(self) -> TlsOptions {
        TlsOptions {
            inner: TlsOptionsKind::Rustls(self.backend),
            shared: self.shared,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use rustls::crypto::aws_lc_rs;

    use super::*;
    use crate::backend::RustlsDefaults;
    use crate::client_identity::ClientIdentity;
    use crate::testing::{AcceptAllServerCertVerifier as AcceptAll, NoClientCertResolver as StubResolver};

    fn provider() -> Arc<rustls::crypto::CryptoProvider> {
        Arc::new(aws_lc_rs::default_provider())
    }

    fn defaults() -> RustlsDefaults {
        RustlsDefaults {
            crypto_provider: provider(),
            verifier: Arc::new(AcceptAll),
        }
    }

    fn shared_with(identity: Option<ClientIdentity>) -> SharedOptions {
        SharedOptions {
            client_identity: identity,
            ..SharedOptions::default()
        }
    }

    #[test]
    fn new_defaults_to_none() {
        let rustls = RustlsOptions::new();
        assert!(rustls.crypto_provider.is_none());
        assert!(rustls.verifier_factory.is_none());
        assert!(rustls.client_identity_resolver.is_none());
    }

    #[test]
    fn builder_rustls_returns_rustls_backend() {
        let builder = TlsOptions::builder_rustls();
        assert!(builder.backend.crypto_provider.is_none());
        assert!(builder.backend.verifier_factory.is_none());
        assert!(builder.backend.client_identity_resolver.is_none());
    }

    #[test]
    fn server_certificate_verifier_stores_verifier() {
        let builder = TlsOptions::builder_rustls().server_certificate_verifier(|_| Arc::new(AcceptAll));
        assert!(builder.backend.verifier_factory.is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn server_certificate_verifier_factory_receives_provider() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static CALLED: AtomicBool = AtomicBool::new(false);
        let rustls_backend = RustlsOptions {
            crypto_provider: Some(provider()),
            verifier_factory: Some(Arc::new(|_provider| {
                CALLED.store(true, Ordering::SeqCst);
                Arc::new(AcceptAll)
            })),
            client_identity_resolver: None,
        };
        rustls_backend.build(None, &shared_with(None)).unwrap();
        assert!(CALLED.load(Ordering::SeqCst));
    }

    #[test]
    fn crypto_provider_stores_provider() {
        let builder = TlsOptions::builder_rustls().crypto_provider(provider());
        assert!(builder.backend.crypto_provider.is_some());
    }

    #[test]
    fn client_identity_sets_identity_in_shared() {
        let identity = ClientIdentity::from_der(vec![vec![0x30u8, 0x00]], vec![0x30u8, 0x00]);
        let builder = TlsOptions::builder_rustls().client_identity(identity);
        assert!(builder.shared.client_identity.is_some());
    }

    #[test]
    fn rustls_build_produces_tls_options() {
        let tls = TlsOptions::builder_rustls().build();
        assert!(matches!(tls.inner, TlsOptionsKind::Rustls(_)));
    }

    #[test]
    fn new_rustls_produces_rustls_tls_options() {
        let tls = TlsOptions::new_rustls();
        assert!(matches!(tls.inner, TlsOptionsKind::Rustls(_)));
        assert!(tls.shared.client_identity.is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // crypto provider FFI (aws-lc-rs) does not run under Miri
    fn build_produces_client_config_without_identity() {
        let _config = TlsOptions::builder_rustls()
            .server_certificate_verifier(|_| Arc::new(AcceptAll))
            .build();
        // Re-build the underlying backend (since `.build()` consumed it) to
        // also exercise the path that produces `rustls::ClientConfig`.
        let rustls_backend = RustlsOptions {
            crypto_provider: None,
            verifier_factory: Some(Arc::new(|_| Arc::new(AcceptAll))),
            client_identity_resolver: None,
        };
        rustls_backend.build(Some(&defaults()), &shared_with(None)).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_returns_error_on_invalid_client_identity() {
        let identity = ClientIdentity::from_der(vec![vec![0x30u8, 0x00]], vec![0x30u8, 0x00]);
        let rustls_backend = RustlsOptions {
            crypto_provider: None,
            verifier_factory: Some(Arc::new(|_| Arc::new(AcceptAll))),
            client_identity_resolver: None,
        };
        let err = rustls_backend.build(Some(&defaults()), &shared_with(Some(identity))).unwrap_err();
        // Surface a debug-format check so we know the underlying rustls error is wrapped.
        let msg = format!("{err}");
        assert!(!msg.is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_falls_back_to_default_verifier() {
        RustlsOptions::new().build(Some(&defaults()), &shared_with(None)).unwrap();
    }

    #[test]
    fn build_without_crypto_provider_returns_error() {
        let err = RustlsOptions::new().build(None, &shared_with(None)).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("crypto provider"), "unexpected error: {msg}");
    }

    #[test]
    fn build_without_verifier_returns_error() {
        // Crypto provider supplied via builder, but no verifier source.
        let rustls_backend = RustlsOptions {
            crypto_provider: Some(provider()),
            verifier_factory: None,
            client_identity_resolver: None,
        };
        let err = rustls_backend.build(None, &shared_with(None)).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("server certificate verifier"), "unexpected error: {msg}");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_uses_builder_crypto_provider_without_defaults() {
        let rustls_backend = RustlsOptions {
            crypto_provider: Some(provider()),
            verifier_factory: Some(Arc::new(|_| Arc::new(AcceptAll))),
            client_identity_resolver: None,
        };
        rustls_backend.build(None, &shared_with(None)).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn tls_options_from_client_config_wraps_as_preconfigured() {
        let config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAll))
            .with_no_client_auth();
        let tls = TlsOptions::from(config);
        assert!(matches!(tls.inner, TlsOptionsKind::PreConfigured(_)));
    }

    #[test]
    fn debug_renders_without_panic() {
        let rustls = RustlsOptions {
            crypto_provider: Some(provider()),
            verifier_factory: Some(Arc::new(|_| Arc::new(AcceptAll))),
            client_identity_resolver: Some(Arc::new(StubResolver)),
        };
        let s = format!("{rustls:?}");
        assert!(s.contains("RustlsOptions"));
        assert!(s.contains("<custom CryptoProvider>"));
        assert!(s.contains("<custom verifier factory>"));
        assert!(s.contains("<custom resolver>"));
    }

    #[test]
    fn client_identity_resolver_stores_resolver() {
        let builder = TlsOptions::builder_rustls().client_identity_resolver(Arc::new(StubResolver));
        assert!(builder.backend.client_identity_resolver.is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_uses_resolver_for_client_auth() {
        let rustls_backend = RustlsOptions {
            crypto_provider: None,
            verifier_factory: Some(Arc::new(|_| Arc::new(AcceptAll))),
            client_identity_resolver: Some(Arc::new(StubResolver)),
        };
        rustls_backend.build(Some(&defaults()), &shared_with(None)).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_resolver_takes_precedence_over_identity() {
        // Identity bytes are intentionally invalid; if precedence is wrong,
        // build would error trying to parse them. The resolver path skips
        // identity parsing entirely.
        let identity = ClientIdentity::from_der(vec![vec![0x30u8, 0x00]], vec![0x30u8, 0x00]);
        let rustls_backend = RustlsOptions {
            crypto_provider: None,
            verifier_factory: Some(Arc::new(|_| Arc::new(AcceptAll))),
            client_identity_resolver: Some(Arc::new(StubResolver)),
        };
        rustls_backend.build(Some(&defaults()), &shared_with(Some(identity))).unwrap();
    }
}
