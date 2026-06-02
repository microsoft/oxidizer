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
/// `TlsOptions` describes the TLS behavior an application wants without
/// committing to a particular implementation. There are a few ways to
/// construct one:
///
/// - With [`TlsOptions::builder`], a backend-agnostic builder that exposes
///   only the configuration options common to every backend. The actual
///   backend is chosen by the consuming library's [`TlsBackendBuilder`].
///   This is the recommended path when an application does not care which
///   backend ends up being used.
/// - With one of the backend-specific constructors (for example, a
///   `new_rustls` / `new_native_tls` helper available when the matching
///   Cargo feature is enabled) for sensible defaults.
/// - With a backend-specific builder (a `builder_rustls` /
///   `builder_native_tls` helper, also feature-gated) when you need to
///   customize backend-specific knobs such as the rustls certificate
///   verifier or client-cert resolver. The builder type is
///   [`TlsOptionsBuilder`].
/// - By wrapping a pre-built `rustls::ClientConfig` or
///   `native_tls::TlsConnector` via `From`/`Into`.
/// - With [`TlsOptions::default`], which leaves the backend choice to the
///   consuming library and uses default values for every shared option.
///
/// # Examples
///
/// Backend-agnostic options that customize only the shared knobs. The
/// consuming library's [`TlsBackendBuilder`] picks the backend:
///
/// ```rust
/// use fetch_tls::TlsOptions;
/// use http::Version;
///
/// let tls = TlsOptions::builder()
///     .supported_http_versions(&[Version::HTTP_2])
///     .build();
/// ```
///
/// Minimal rustls-backed options using defaults. The consuming library is
/// expected to have configured the rustls crypto provider on its
/// [`TlsBackendBuilder`] before materializing this into a backend:
///
/// ```rust,no_run
/// # #[cfg(feature = "rustls")] {
/// use fetch_tls::TlsOptions;
///
/// let tls = TlsOptions::new_rustls();
/// # }
/// ```
///
/// Minimal native-tls-backed options using defaults; no environment-
/// supplied defaults are required to materialize the backend:
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

/// Constructs [`TlsOptions`] whose backend is chosen when the consuming
/// library materializes them via its [`TlsBackendBuilder`]. See
/// [`TlsBackendBuilder`] for how to configure the default backend.
impl Default for TlsOptions {
    fn default() -> Self {
        Self {
            inner: TlsOptionsKind::Auto,
            shared: SharedOptions::default(),
        }
    }
}

/// Marker for [`TlsOptionsBuilder`] returned by [`TlsOptions::builder`].
///
/// Carries no backend-specific state: the consuming library's
/// [`TlsBackendBuilder`] decides which backend to use when the resulting
/// [`TlsOptions`] is materialized.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct AutoBackend;

impl TlsOptions {
    /// Creates a backend-agnostic builder for [`TlsOptions`].
    ///
    /// The builder exposes only options that apply to every backend (such
    /// as supported HTTP versions and the client identity for `mTLS`).
    /// The actual backend is selected by the consuming library's
    /// [`TlsBackendBuilder`] when the options are materialized, so callers
    /// don't need to pick `builder_rustls` / `builder_native_tls`
    /// explicitly.
    pub fn builder() -> TlsOptionsBuilder<AutoBackend> {
        TlsOptionsBuilder {
            backend: AutoBackend,
            shared: SharedOptions::default(),
        }
    }
}

impl TlsOptionsBuilder<AutoBackend> {
    /// Builds the final [`TlsOptions`] without pinning a backend.
    ///
    /// The consuming library's [`TlsBackendBuilder`] picks the backend
    /// when [`TlsBackendBuilder::build_backend`](crate::TlsBackendBuilder::build_backend)
    /// is called.
    pub fn build(self) -> TlsOptions {
        TlsOptions {
            inner: TlsOptionsKind::Auto,
            shared: self.shared,
        }
    }
}

/// Type-state builder for [`TlsOptions`], parameterized by backend.
///
/// The type parameter `B` carries the backend-specific state (rustls or
/// native-tls). Obtain a builder from one of the feature-gated
/// `TlsOptions::builder_*` constructors and finish with `.build()`.
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
    /// conversion happens when the options are materialized into a backend.
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
    fn builder_returns_auto_variant() {
        let tls = TlsOptions::builder().build();
        assert!(matches!(tls.inner, TlsOptionsKind::Auto));
        assert!(tls.shared.supported_http_versions.is_none());
        assert!(tls.shared.client_identity.is_none());
    }

    #[test]
    fn builder_supports_shared_options() {
        let tls = TlsOptions::builder().supported_http_versions(&[Version::HTTP_2]).build();
        assert!(matches!(tls.inner, TlsOptionsKind::Auto));
        assert_eq!(tls.shared.supported_http_versions.as_deref(), Some(&[Version::HTTP_2][..]));
    }

    #[test]
    fn supported_http_versions_stores_provided_versions() {
        let builder = TlsOptions::builder_rustls().supported_http_versions(&[Version::HTTP_2]);
        assert_eq!(builder.shared.supported_http_versions.as_deref(), Some(&[Version::HTTP_2][..]));
    }

    #[test]
    fn supported_http_versions_overwrites_previous_value() {
        let builder = TlsOptions::builder_rustls()
            .supported_http_versions(&[Version::HTTP_11])
            .supported_http_versions(&[Version::HTTP_2, Version::HTTP_11]);
        assert_eq!(
            builder.shared.supported_http_versions.as_deref(),
            Some(&[Version::HTTP_2, Version::HTTP_11][..])
        );
    }

    #[test]
    fn supported_http_versions_panics_when_empty() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = TlsOptions::builder_rustls().supported_http_versions(&[]);
        }));
        assert!(result.is_err());
    }
}
