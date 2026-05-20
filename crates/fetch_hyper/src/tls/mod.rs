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
