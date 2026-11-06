// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! TLS backends for HTTP clients.

#[cfg(any(feature = "native-tls", test))]
use std::collections::HashMap;
#[cfg(any(feature = "native-tls", test))]
use std::sync::Arc;

#[cfg(any(feature = "native-tls", test))]
use http::uri::Authority;

/// An error from building a [`TlsBackend`] from [`TlsOptions`](super::TlsOptions).
#[ohno::error]
pub struct BackendError;

/// A platform native TLS backend.
///
/// Contains the platform connector and server name overrides for TLS handshakes.
#[cfg(any(feature = "native-tls", test))]
#[derive(Debug, Clone)]
pub struct NativeTlsBackend {
    connector: ::native_tls::TlsConnector,
    server_name_overrides: Arc<HashMap<String, String>>,
}

#[cfg(any(feature = "native-tls", test))]
impl NativeTlsBackend {
    pub(crate) fn new(connector: ::native_tls::TlsConnector, server_name_overrides: HashMap<String, String>) -> Self {
        Self {
            connector,
            server_name_overrides: Arc::new(server_name_overrides),
        }
    }

    /// Returns the underlying platform TLS connector.
    #[must_use]
    pub fn connector(&self) -> &::native_tls::TlsConnector {
        &self.connector
    }

    /// Returns the TLS server name override for an endpoint.
    ///
    /// An exact authority match includes the port and takes priority over a
    /// host-only override.
    #[must_use]
    pub fn server_name_for(&self, endpoint: &Authority) -> Option<&str> {
        self.server_name_overrides
            .get(&server_name_override_key(endpoint))
            .or_else(|| {
                self.server_name_overrides
                    .get(&endpoint.host().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase())
            })
            .map(String::as_str)
    }
}

#[cfg(any(feature = "native-tls", test))]
pub(crate) fn server_name_override_key(endpoint: &Authority) -> String {
    let host = endpoint.host().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    match endpoint.port_u16() {
        Some(port) if host.contains(':') => format!("[{host}]:{port}"),
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// A TLS backend that an HTTP client can use.
///
/// [`TlsOptions`](super::TlsOptions) describes the required TLS configuration.
/// `TlsBackend` contains the built configuration. The enabled Cargo features
/// determine which variants are available.
///
/// Use [`TlsBackendBuilder`](super::TlsBackendBuilder) to build this type.
/// Construct it directly only to wrap a backend that you already built.
#[derive(Debug, Clone)]
#[allow(
    clippy::allow_attributes,
    clippy::large_enum_variant,
    reason = "this configuration object stays inline because boxing complicates the public API without a performance benefit"
)]
pub enum TlsBackend {
    /// A rustls backend with a shared `rustls::ClientConfig`.
    #[cfg(any(feature = "rustls", test))]
    Rustls(std::sync::Arc<::rustls::ClientConfig>),

    /// A platform native TLS backend.
    ///
    /// The backend uses `SChannel` on Windows, Security Framework on `macOS`,
    /// and `OpenSSL` on Linux.
    ///
    /// # Migration from `fetch_tls` 0.4
    ///
    /// This variant contains [`NativeTlsBackend`] instead of
    /// [`native_tls::TlsConnector`]. Use `connector.into()` to construct it.
    /// Use [`NativeTlsBackend::connector`] to access the platform connector.
    #[cfg(any(feature = "native-tls", test))]
    NativeTls(NativeTlsBackend),
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
impl From<::native_tls::TlsConnector> for NativeTlsBackend {
    fn from(connector: ::native_tls::TlsConnector) -> Self {
        Self::new(connector, HashMap::new())
    }
}

#[cfg(any(feature = "native-tls", test))]
impl From<::native_tls::TlsConnector> for TlsBackend {
    fn from(connector: ::native_tls::TlsConnector) -> Self {
        Self::NativeTls(connector.into())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls uses platform TLS FFI
    fn native_tls_backend_prefers_exact_authority_over_host() {
        let connector = ::native_tls::TlsConnector::new().unwrap();
        let backend = NativeTlsBackend::new(
            connector,
            HashMap::from([
                ("localhost".to_owned(), "host.example.test".to_owned()),
                ("localhost:8443".to_owned(), "endpoint.example.test".to_owned()),
            ]),
        );

        assert_eq!(
            backend.server_name_for(&"localhost:8443".parse().unwrap()),
            Some("endpoint.example.test")
        );
        assert_eq!(
            backend.server_name_for(&"localhost:9443".parse().unwrap()),
            Some("host.example.test")
        );
        assert_eq!(backend.server_name_for(&"example.com".parse().unwrap()), None);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls uses platform TLS FFI
    fn native_tls_backend_matches_host_case_insensitively() {
        let connector = ::native_tls::TlsConnector::new().unwrap();
        let backend = NativeTlsBackend::new(
            connector,
            HashMap::from([
                ("example.com".to_owned(), "host.example.test".to_owned()),
                ("example.com:8443".to_owned(), "endpoint.example.test".to_owned()),
            ]),
        );

        assert_eq!(
            backend.server_name_for(&"EXAMPLE.COM:8443".parse().unwrap()),
            Some("endpoint.example.test")
        );
        assert_eq!(
            backend.server_name_for(&"EXAMPLE.COM:9443".parse().unwrap()),
            Some("host.example.test")
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls uses platform TLS FFI
    fn native_tls_backend_preserves_ipv6_brackets_in_exact_key() {
        let connector = ::native_tls::TlsConnector::new().unwrap();
        let backend = NativeTlsBackend::new(
            connector,
            HashMap::from([("[::1]:8443".to_owned(), "loopback.example.test".to_owned())]),
        );

        assert_eq!(
            backend.server_name_for(&"[::1]:8443".parse().unwrap()),
            Some("loopback.example.test")
        );
    }

    #[test]
    fn native_tls_backend_is_unwind_safe() {
        static_assertions::assert_impl_all!(
            NativeTlsBackend: std::panic::UnwindSafe,
            std::panic::RefUnwindSafe
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls uses platform TLS FFI
    fn native_tls_connector_converts_to_backend_payload() {
        let backend: NativeTlsBackend = ::native_tls::TlsConnector::new().unwrap().into();
        assert!(backend.server_name_overrides.is_empty());
    }
}
