// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TlsOptions`] and its type-state builder [`TlsOptionsBuilder`].

use http::Version;

use crate::client_identity::ClientIdentity;
use crate::{TlsBackend, TlsBackendBuilder};

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
    /// is configured via [`TlsBackendBuilder`]; see its documentation for details.
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
/// [`TlsBackendBuilder::configure_rustls`](crate::TlsBackendBuilder::configure_rustls)
/// when calling [`TlsBackendBuilder::build_backend`](crate::TlsBackendBuilder::build_backend) to materialize a backend:
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
    pub(crate) inner: TlsOptionsKind,
    pub(crate) shared: SharedOptions,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SharedOptions {
    pub(crate) supported_http_versions: Option<Vec<Version>>,
    pub(crate) client_identity: Option<ClientIdentity>,
}

impl SharedOptions {
    #[allow(
        clippy::allow_attributes,
        dead_code,
        reason = "used by feature-gated backend builders; can be unused in builds without rustls/native-tls"
    )]
    pub(crate) fn resolved_supported_http_versions<'a>(&'a self, defaults: &'a TlsBackendBuilder) -> &'a [Version] {
        self.supported_http_versions
            .as_deref()
            .unwrap_or(defaults.supported_http_versions.as_slice())
    }
}

/// Constructs [`TlsOptions`] whose backend is chosen at
/// [`TlsBackendBuilder::build_backend`](crate::TlsBackendBuilder::build_backend) time from the supplied
/// [`TlsBackendBuilder`]. See [`TlsBackendBuilder`] for how to select the
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
        self.shared.supported_http_versions = Some(versions.to_vec());
        self
    }

    /// Sets the client identity for mutual TLS (`mTLS`) authentication.
    ///
    /// The same identity works for either backend; backend-specific
    /// conversion happens in [`TlsBackendBuilder::build_backend`](crate::TlsBackendBuilder::build_backend).
    /// The native-tls backend requires the private key to be `PKCS#8`.
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
    fn default_supported_http_versions_is_not_set() {
        let shared = SharedOptions::default();
        assert!(shared.supported_http_versions.is_none());
    }

    #[test]
    fn resolved_supported_http_versions_falls_back_to_backend_defaults() {
        let shared = SharedOptions::default();
        let defaults = TlsBackendBuilder::new();

        assert_eq!(
            shared.resolved_supported_http_versions(&defaults),
            [Version::HTTP_11, Version::HTTP_2]
        );
    }

    #[test]
    fn default_constructs_auto_variant() {
        let tls = TlsOptions::default();
        assert!(matches!(tls.inner, TlsOptionsKind::Auto));
    }

    #[test]
    fn supported_http_versions_panics_when_empty() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = TlsOptions::builder_rustls().supported_http_versions(&[]);
        }));
        assert!(result.is_err());
    }
}
