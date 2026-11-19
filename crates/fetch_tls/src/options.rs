// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TlsOptions`] and the [`TlsOptionsBuilder`] type-state builder.

use http::Version;

use crate::client_identity::ClientIdentity;
use crate::{TlsBackend, TlsBackendBuilder};

/// Internal representation of a TLS configuration.
#[derive(Debug, Clone)]
#[allow(
    clippy::allow_attributes,
    clippy::large_enum_variant,
    dead_code,
    reason = "downstream HTTP client crates consume these configuration variants to build TLS backends from TlsOptions"
)]
pub(crate) enum TlsOptionsKind {
    /// [`TlsBackendBuilder`] selects its configured default backend.
    Auto,

    /// A pre-configured TLS backend that is used without changes.
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
/// `TlsOptions` describes TLS requirements. It can select a backend or let the
/// client library select one. You can construct it in these ways:
///
/// - [`TlsOptions::builder`] sets values that apply to every backend.
///   [`TlsBackendBuilder`] selects the backend.
/// - `new_rustls` and `new_native_tls` create backend-specific defaults when
///   the matching Cargo feature is enabled.
/// - `builder_rustls` and `builder_native_tls` set backend-specific values when
///   the matching Cargo feature is enabled.
/// - `From` and `Into` wrap a pre-built `rustls::ClientConfig` or
///   `native_tls::TlsConnector` when the matching Cargo feature is enabled.
/// - [`TlsOptions::default`] uses shared defaults and lets the client library
///   select the backend.
///
/// # Examples
///
/// These options set an HTTP version and let [`TlsBackendBuilder`] select the
/// backend:
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
/// These rustls options use defaults. The client library must configure the
/// rustls crypto provider on [`TlsBackendBuilder`] before it builds the backend:
///
/// ```rust,no_run
/// # #[cfg(feature = "rustls")] {
/// use fetch_tls::TlsOptions;
///
/// let tls = TlsOptions::new_rustls();
/// # }
/// ```
///
/// These native TLS options use defaults. They do not need environment values:
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
        reason = "feature-gated backend builders use this method. Builds without backend features do not use it"
    )]
    pub(crate) fn resolved_supported_http_versions<'a>(&'a self, defaults: &'a TlsBackendBuilder) -> &'a [Version] {
        self.supported_http_versions
            .as_deref()
            .unwrap_or(defaults.supported_http_versions.as_slice())
    }
}

/// Creates [`TlsOptions`] that lets [`TlsBackendBuilder`] select the backend.
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
/// This type contains no backend-specific values. [`TlsBackendBuilder`] selects
/// the backend when it builds the resulting [`TlsOptions`].
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct AutoBackend;

impl TlsOptions {
    /// Creates a backend-agnostic builder for [`TlsOptions`].
    ///
    /// The builder contains only values that apply to every backend, such as
    /// HTTP versions and the `mTLS` client identity. [`TlsBackendBuilder`]
    /// selects the backend.
    pub fn builder() -> TlsOptionsBuilder<AutoBackend> {
        TlsOptionsBuilder {
            backend: AutoBackend,
            shared: SharedOptions::default(),
        }
    }
}

impl TlsOptionsBuilder<AutoBackend> {
    /// Builds [`TlsOptions`] without selecting a backend.
    ///
    /// [`TlsBackendBuilder::build_backend`](crate::TlsBackendBuilder::build_backend)
    /// selects the backend.
    pub fn build(self) -> TlsOptions {
        TlsOptions {
            inner: TlsOptionsKind::Auto,
            shared: self.shared,
        }
    }
}

/// A type-state builder for [`TlsOptions`].
///
/// The `B` type contains backend-specific values. Get a builder from a
/// `TlsOptions::builder_*` method and finish with `.build()`.
#[derive(Debug, Clone)]
#[must_use]
pub struct TlsOptionsBuilder<B> {
    #[allow(
        clippy::allow_attributes,
        dead_code,
        reason = "feature-gated rustls and native-tls implementations read this field. It is unused when both features are disabled"
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
            "supported_http_versions cannot be empty. Configure at least one HTTP version, such as HTTP/1.1 or HTTP/2"
        );
        self.shared.supported_http_versions = Some(versions.to_vec());
        self
    }

    /// Sets the client identity for mutual TLS (`mTLS`) authentication.
    ///
    /// The same identity works with either backend. Each backend converts it
    /// when [`TlsBackendBuilder`] builds the backend.
    /// The native-tls backend requires a `PKCS#8` private key. It does not
    /// support client identities on Apple platforms other than macOS.
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
