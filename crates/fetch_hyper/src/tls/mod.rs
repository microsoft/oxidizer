// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `TLS` backend selection and internal connector wiring.
//!
//! The only public symbol is [`TlsBackend`]; everything else is internal.

mod connector;
pub(crate) use connector::TlsConnector;

/// Selects and supplies the `TLS` backend used by the transport.
///
/// Construct from a backend-specific configuration:
///
/// ```ignore
/// use std::sync::Arc;
/// use rustls::ClientConfig;
/// use fetch_hyper::TlsBackend;
///
/// # fn make_config() -> ClientConfig { unimplemented!() }
/// let backend: TlsBackend = make_config().into();
/// // or
/// let backend = TlsBackend::Rustls(Arc::new(make_config()));
/// ```
///
/// When neither the `rustls` nor `native-tls` feature is enabled this enum
/// has no variants and is therefore uninhabited: the crate still compiles,
/// but a [`TlsBackend`] value cannot be constructed and the transport
/// cannot be used. Enable at least one `TLS` feature to make outbound
/// connections.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TlsBackend {
    /// Use the `rustls` backend with the given pre-built configuration.
    #[cfg(feature = "rustls")]
    Rustls(std::sync::Arc<rustls::ClientConfig>),

    /// Use the platform `native-tls` backend with the given connector.
    #[cfg(feature = "native-tls")]
    NativeTls(native_tls::TlsConnector),
}

#[cfg(feature = "rustls")]
impl From<rustls::ClientConfig> for TlsBackend {
    fn from(config: rustls::ClientConfig) -> Self {
        Self::Rustls(std::sync::Arc::new(config))
    }
}

#[cfg(feature = "rustls")]
impl From<std::sync::Arc<rustls::ClientConfig>> for TlsBackend {
    fn from(config: std::sync::Arc<rustls::ClientConfig>) -> Self {
        Self::Rustls(config)
    }
}

#[cfg(feature = "native-tls")]
impl From<native_tls::TlsConnector> for TlsBackend {
    fn from(connector: native_tls::TlsConnector) -> Self {
        Self::NativeTls(connector)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[cfg(feature = "rustls")]
    #[test]
    fn from_client_config_makes_rustls_variant() {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .unwrap_or_else(|| std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let backend: TlsBackend = config.into();
        assert!(matches!(backend, TlsBackend::Rustls(_)));
    }

    #[cfg(feature = "rustls")]
    #[test]
    fn from_arc_client_config_makes_rustls_variant() {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .unwrap_or_else(|| std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
        let config = std::sync::Arc::new(
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        );
        let backend: TlsBackend = config.into();
        assert!(matches!(backend, TlsBackend::Rustls(_)));
    }

    #[cfg(feature = "native-tls")]
    #[test]
    fn from_native_tls_connector_makes_native_variant() {
        let nc = native_tls::TlsConnector::new().expect("default native-tls connector should build");
        let backend: TlsBackend = nc.into();
        assert!(matches!(backend, TlsBackend::NativeTls(_)));
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn clone_preserves_variant() {
        #[cfg(feature = "native-tls")]
        {
            let nc = native_tls::TlsConnector::new().unwrap();
            let backend = TlsBackend::NativeTls(nc);
            assert!(matches!(backend, TlsBackend::NativeTls(_)));
        }
    }
}
