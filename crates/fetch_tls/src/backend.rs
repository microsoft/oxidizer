// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fully constructed TLS backends ready for use by an HTTP client.

#[cfg(any(feature = "native-tls", test))]
use std::collections::HashMap;
#[cfg(any(feature = "native-tls", test))]
use std::sync::Arc;

#[cfg(any(feature = "native-tls", test))]
use http::uri::Authority;

/// Error returned when materializing a [`TlsBackend`] from
/// [`TlsOptions`](super::TlsOptions) fails.
#[ohno::error]
pub struct BackendError;

/// A platform native TLS backend.
///
/// The backend carries the platform connector together with any
/// endpoint-specific server name overrides used during handshakes.
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

    /// Returns the configured TLS server name for an endpoint.
    ///
    /// An exact authority match, including the port when present, takes
    /// precedence over a host-only override.
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

/// A fully constructed TLS backend ready for use by an HTTP client.
///
/// Where [`TlsOptions`](super::TlsOptions) describes *how* to build a TLS
/// configuration, `TlsBackend` holds the result. Which variants are
/// available depends on the enabled Cargo features.
///
/// Typically produced by [`TlsBackendBuilder`](super::TlsBackendBuilder);
/// construct directly only when wrapping a backend you have already built.
#[derive(Debug, Clone)]
#[allow(
    clippy::allow_attributes,
    clippy::large_enum_variant,
    reason = "configuration object; boxing would clutter the public API without performance benefit"
)]
pub enum TlsBackend {
    /// rustls backend, carrying a shared `rustls::ClientConfig`.
    #[cfg(any(feature = "rustls", test))]
    Rustls(std::sync::Arc<::rustls::ClientConfig>),

    /// Platform native TLS backend (`SChannel` on Windows, Security Framework
    /// on `macOS`, `OpenSSL` on Linux).
    ///
    /// # Migration from `fetch_tls` 0.4
    ///
    /// This variant now contains [`NativeTlsBackend`] instead of
    /// [`native_tls::TlsConnector`]. Construct it with `connector.into()`.
    /// After destructuring the variant, access the platform connector with
    /// [`NativeTlsBackend::connector`].
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
impl From<::native_tls::TlsConnector> for TlsBackend {
    fn from(connector: ::native_tls::TlsConnector) -> Self {
        Self::NativeTls(NativeTlsBackend::new(connector, HashMap::new()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls touches platform TLS FFI
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
    #[cfg_attr(miri, ignore)] // native-tls touches platform TLS FFI
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
    #[cfg_attr(miri, ignore)] // native-tls touches platform TLS FFI
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
}
