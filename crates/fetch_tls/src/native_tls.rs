// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform native TLS backend configuration and builder integration.

use http::Version;
use native_tls::TlsConnector;

use crate::backend::BackendError;
use crate::options::{SharedOptions, TlsOptions, TlsOptionsBuilder, TlsOptionsKind};

// Application-Layer Protocol Negotiation identifiers; see
// <https://en.wikipedia.org/wiki/Application-Layer_Protocol_Negotiation>.
const HTTP_11_ALPN: &str = "http/1.1";
const HTTP_2_ALPN: &str = "h2";

/// Platform native TLS backend.
#[derive(Clone)]
#[non_exhaustive]
pub struct NativeTlsOptions;

impl std::fmt::Debug for NativeTlsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeTlsOptions").finish()
    }
}

impl NativeTlsOptions {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Materializes this configuration into a [`native_tls::TlsConnector`].
    #[expect(clippy::unused_self, reason = "method takes self for symmetry with RustlsOptions::build")]
    pub(crate) fn build(self, shared: &SharedOptions) -> Result<TlsConnector, BackendError> {
        let mut builder = native_tls::TlsConnector::builder();
        builder
            .request_alpns(map_to_alpn(&shared.supported_http_versions))
            .min_protocol_version(Some(native_tls::Protocol::Tlsv12));

        if let Some(identity) = shared.client_identity.as_ref() {
            identity.build_native_identity()
                .map(|i| { builder.identity(i); })
                .map_err(BackendError::caused_by)?;
        }

        builder.build().map_err(BackendError::caused_by)
    }
}

fn map_to_alpn(versions: &[Version]) -> &[&str] {
    let http1 = versions.contains(&Version::HTTP_11) || versions.contains(&Version::HTTP_10);
    let http2 = versions.contains(&Version::HTTP_2);
    if http2 && http1 {
        &[HTTP_2_ALPN, HTTP_11_ALPN]
    } else if http2 {
        &[HTTP_2_ALPN]
    } else if http1 {
        &[HTTP_11_ALPN]
    } else {
        &[]
    }
}

impl TlsOptions {
    /// Creates a builder for the platform native TLS backend.
    pub fn builder_native_tls() -> TlsOptionsBuilder<NativeTlsOptions> {
        TlsOptionsBuilder {
            backend: NativeTlsOptions::new(),
            shared: SharedOptions::default(),
        }
    }

    /// Creates [`TlsOptions`] for the platform native TLS backend using
    /// default settings.
    ///
    /// Equivalent to `TlsOptions::builder_native_tls().build()`. Use
    /// [`TlsOptions::builder_native_tls`] when you need to customize the
    /// configuration before building.
    pub fn new_native_tls() -> Self {
        Self::builder_native_tls().build()
    }
}

/// Wraps a pre-built [`native_tls::TlsConnector`] as [`TlsOptions`].
impl From<TlsConnector> for TlsOptions {
    fn from(connector: TlsConnector) -> Self {
        Self {
            inner: TlsOptionsKind::PreConfigured(connector.into()),
            shared: SharedOptions::default(),
        }
    }
}

impl TlsOptionsBuilder<NativeTlsOptions> {
    /// Builds the final [`TlsOptions`] with the native TLS backend.
    pub fn build(self) -> TlsOptions {
        TlsOptions {
            inner: TlsOptionsKind::NativeTls(self.backend),
            shared: self.shared,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::client_identity::ClientIdentity;

    #[test]
    fn builder_native_tls_starts_empty() {
        let builder = TlsOptions::builder_native_tls();
        assert!(builder.shared.client_identity.is_none());
    }

    #[test]
    fn build_produces_native_tls_options() {
        let tls = TlsOptions::builder_native_tls().build();
        assert!(matches!(tls.inner, TlsOptionsKind::NativeTls(_)));
    }

    #[test]
    fn new_native_tls_produces_native_tls_options() {
        let tls = TlsOptions::new_native_tls();
        assert!(matches!(tls.inner, TlsOptionsKind::NativeTls(_)));
        assert!(tls.shared.client_identity.is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls touches platform TLS FFI
    fn tls_options_from_tls_connector_wraps_as_preconfigured() {
        let connector = native_tls::TlsConnector::builder().build().expect("builds");
        let tls = TlsOptions::from(connector);
        assert!(matches!(tls.inner, TlsOptionsKind::PreConfigured(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_fails_for_invalid_client_identity() {
        let identity = ClientIdentity::from_der(vec![vec![0xffu8, 0xff]], vec![0x30u8, 0x00]);
        let tls = TlsOptions::builder_native_tls().client_identity(identity).build();
        tls.build_backend(&crate::TlsBackendDefaults::new())
            .expect_err("expected build_backend to fail for invalid certificate");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_produces_tls_connector() {
        NativeTlsOptions::new().build(&SharedOptions::default()).unwrap();
    }

    #[test]
    fn map_to_alpn_http1_and_http2() {
        assert_eq!(map_to_alpn(&[Version::HTTP_11, Version::HTTP_2]), &["h2", "http/1.1"]);
    }

    #[test]
    fn map_to_alpn_http2_only() {
        assert_eq!(map_to_alpn(&[Version::HTTP_2]), &["h2"]);
    }

    #[test]
    fn map_to_alpn_http11_only() {
        assert_eq!(map_to_alpn(&[Version::HTTP_11]), &["http/1.1"]);
    }

    #[test]
    fn map_to_alpn_http10_aliases_to_http1() {
        assert_eq!(map_to_alpn(&[Version::HTTP_10]), &["http/1.1"]);
    }

    #[test]
    fn map_to_alpn_empty() {
        let empty: &[&str] = &[];
        assert_eq!(map_to_alpn(&[]), empty);
    }

    #[test]
    fn map_to_alpn_http3_only_is_empty() {
        let empty: &[&str] = &[];
        assert_eq!(map_to_alpn(&[Version::HTTP_3]), empty);
    }

    #[test]
    fn map_to_alpn_http10_and_http2() {
        assert_eq!(map_to_alpn(&[Version::HTTP_10, Version::HTTP_2]), &["h2", "http/1.1"]);
    }

    #[test]
    fn debug_renders_presence_only() {
        let s = format!("{:?}", NativeTlsOptions::new());
        assert!(s.contains("NativeTlsOptions"));
    }
}
