// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An enum that wraps the `TLS` connector, dispatching to the configured backend.

use std::marker::PhantomData;
#[cfg(any(feature = "rustls", test))]
use std::sync::Arc;

use fetch_tls::TlsBackend;
use http::Version;
use templated_uri::BaseUri;
#[cfg(any(feature = "rustls", feature = "native-tls", test))]
use tower::Service as _;

#[cfg(any(feature = "rustls", feature = "native-tls", test))]
use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::options::RequestFilter;
use crate::{Connect, HyperIo};

/// An enum that wraps the `TLS` connector, dispatching to the correct backend at runtime.
pub(crate) enum TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    #[cfg(any(feature = "rustls", test))]
    Rustls(hyper_rustls::HttpsConnector<HyperConnectorAdapter<C, S>>, PhantomData<fn() -> S>),

    #[cfg(any(feature = "native-tls", test))]
    NativeTls(hyper_tls::HttpsConnector<HyperConnectorAdapter<C, S>>, PhantomData<fn() -> S>),

    #[cfg(not(any(feature = "rustls", feature = "native-tls", test)))]
    None(PhantomData<fn(C, S)>),
}

impl<C, S> Clone for TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    fn clone(&self) -> Self {
        match self {
            #[cfg(any(feature = "rustls", test))]
            Self::Rustls(c, _) => Self::Rustls(c.clone(), PhantomData),
            #[cfg(any(feature = "native-tls", test))]
            Self::NativeTls(c, _) => Self::NativeTls(c.clone(), PhantomData),
            #[cfg(not(any(feature = "rustls", feature = "native-tls", test)))]
            Self::None(_) => Self::None(PhantomData),
        }
    }
}

impl<C, S> TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    #[expect(clippy::allow_attributes, reason = "expect would be unfulfilled when a TLS feature is enabled")]
    #[allow(
        unused_variables,
        unreachable_patterns,
        clippy::needless_pass_by_value,
        reason = "parameters are consumed only in feature-gated match arms; the fallback `_` arm is unreachable when fetch_tls only carries variants whose features are enabled here"
    )]
    pub(crate) fn new(backend: TlsBackend, connector: C, request_filter: RequestFilter, supported_versions: &[Version]) -> Self {
        match backend {
            #[cfg(any(feature = "rustls", test))]
            TlsBackend::Rustls(config) => Self::Rustls(
                build_rustls_connector(Arc::unwrap_or_clone(config), connector, request_filter, supported_versions),
                PhantomData,
            ),
            #[cfg(any(feature = "native-tls", test))]
            TlsBackend::NativeTls(native) => Self::NativeTls(build_native_tls_connector(native, connector, request_filter), PhantomData),
        }
    }
}

// The internal ALPN selection only manifests through TLS handshakes against
// a real HTTPS server, which is out of scope for these tests; the surviving
// boolean mutations on `http1`/`http2` produce observably identical results
// when the connector is exercised over plain HTTP.
#[cfg(any(feature = "rustls", test))]
#[cfg_attr(test, mutants::skip)]
fn build_rustls_connector<C, S>(
    mut config: rustls::ClientConfig,
    connector: C,
    request_filter: RequestFilter,
    supported_versions: &[Version],
) -> hyper_rustls::HttpsConnector<HyperConnectorAdapter<C, S>>
where
    C: Connect<S>,
    S: HyperIo,
{
    // hyper-rustls expects ALPN to be configured via enable_http1/enable_http2.
    config.alpn_protocols.clear();
    let builder = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(config);

    let builder = match request_filter {
        RequestFilter::Https => builder.https_only(),
        RequestFilter::HttpAndHttps => builder.https_or_http(),
    };

    let inner = HyperConnectorAdapter::new(connector);

    let http1 = supported_versions.contains(&Version::HTTP_11) || supported_versions.contains(&Version::HTTP_10);
    let http2 = supported_versions.contains(&Version::HTTP_2);

    if http1 && http2 {
        builder.enable_http1().enable_http2().wrap_connector(inner)
    } else if http2 {
        builder.enable_http2().wrap_connector(inner)
    } else {
        builder.enable_http1().wrap_connector(inner)
    }
}

#[cfg(any(feature = "native-tls", test))]
fn build_native_tls_connector<C, S>(
    native: native_tls::TlsConnector,
    connector: C,
    request_filter: RequestFilter,
) -> hyper_tls::HttpsConnector<HyperConnectorAdapter<C, S>>
where
    C: Connect<S>,
    S: HyperIo,
{
    let tokio_connector = tokio_native_tls::TlsConnector::from(native);
    let inner = HyperConnectorAdapter::new(connector);
    let mut https = hyper_tls::HttpsConnector::from((inner, tokio_connector));

    https.https_only(matches!(request_filter, RequestFilter::Https));

    https
}

impl<C, S> layered::Service<BaseUri> for TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    type Out = http_extensions::Result<Box<dyn HyperIo>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        match self {
            #[cfg(any(feature = "rustls", test))]
            Self::Rustls(c, _) => {
                let mut c = c.clone();
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::new(s) as Box<dyn HyperIo>)
                    .map_err(handle_tls_error)
            }
            #[cfg(any(feature = "native-tls", test))]
            Self::NativeTls(c, _) => {
                let mut c = c.clone();
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::new(s) as Box<dyn HyperIo>)
                    .map_err(handle_tls_error)
            }
            #[cfg(not(any(feature = "rustls", feature = "native-tls", test)))]
            Self::None(_) => {
                let _ = input;
                unreachable!(
                    "`TlsConnector::None` cannot be constructed because `TlsBackend` is uninhabited when no TLS feature is enabled"
                )
            }
        }
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls", test))]
fn handle_tls_error(e: Box<dyn std::error::Error + Send + Sync>) -> http_extensions::HttpError {
    let recovery = crate::recoverability::detect_recoverability(e.as_ref());
    http_extensions::HttpError::other(e, recovery, crate::error_labels::LABEL_CONNECT)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use bytes::Bytes;
    use layered::Service as _;
    use tick::Clock;

    use super::*;
    use crate::testing::{FakeConnector, FakeStream, TestError};

    fn native_tls_backend() -> TlsBackend {
        TlsBackend::NativeTls(native_tls::TlsConnector::new().unwrap())
    }

    fn rustls_backend() -> TlsBackend {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .unwrap_or_else(|| std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();

        config.into()
    }

    fn fake_connector() -> FakeConnector {
        FakeConnector::new_success(Bytes::new(), Clock::new_frozen())
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_with_native_tls_backend_creates_native_variant() {
        let c: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(native_tls_backend(), fake_connector(), RequestFilter::Https, &[Version::HTTP_11]);
        assert!(matches!(c, TlsConnector::NativeTls(_, _)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_with_native_tls_http_and_https_filter() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend(),
            fake_connector(),
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11],
        );
        assert!(matches!(c, TlsConnector::NativeTls(_, _)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_with_rustls_https_only_filter_and_both_versions() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            rustls_backend(),
            fake_connector(),
            RequestFilter::Https,
            &[Version::HTTP_11, Version::HTTP_2],
        );
        assert!(matches!(c, TlsConnector::Rustls(_, _)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_with_rustls_http_and_https_filter_h2_only() {
        let c: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(rustls_backend(), fake_connector(), RequestFilter::HttpAndHttps, &[Version::HTTP_2]);
        assert!(matches!(c, TlsConnector::Rustls(_, _)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_with_rustls_http1_only_with_http10_alias() {
        let c: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(rustls_backend(), fake_connector(), RequestFilter::Https, &[Version::HTTP_10]);
        assert!(matches!(c, TlsConnector::Rustls(_, _)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn clone_rustls_variant() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            rustls_backend(),
            fake_connector(),
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11, Version::HTTP_2],
        );
        let c2 = c.clone();
        assert!(matches!(c, TlsConnector::Rustls(_, _)));
        assert!(matches!(c2, TlsConnector::Rustls(_, _)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn clone_native_tls_variant() {
        let c: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(native_tls_backend(), fake_connector(), RequestFilter::Https, &[Version::HTTP_11]);
        let c2 = c.clone();
        assert!(matches!(c, TlsConnector::NativeTls(_, _)));
        assert!(matches!(c2, TlsConnector::NativeTls(_, _)));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn execute_native_tls_with_plain_http_returns_stream() {
        // For plain http://, native-tls passes through without performing a handshake.
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend(),
            fake_connector(),
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11],
        );
        let result = c.execute(templated_uri::BaseUri::from_static("http://example.com")).await;
        result.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn execute_rustls_with_plain_http_returns_stream() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            rustls_backend(),
            fake_connector(),
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11, Version::HTTP_2],
        );
        let result = c.execute(templated_uri::BaseUri::from_static("http://example.com")).await;
        result.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn execute_native_tls_propagates_connector_error() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_connect_failure(TestError::new("fail"), clock);
        let c: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(native_tls_backend(), connector, RequestFilter::HttpAndHttps, &[Version::HTTP_11]);
        let result = c.execute(templated_uri::BaseUri::from_static("http://example.com")).await;
        let Err(err) = result else {
            panic!("connector error should propagate");
        };
        assert!(err.to_string().contains("fail"), "got: {err}");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn execute_rustls_propagates_connector_error() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_connect_failure(TestError::new("fail-rustls"), clock);
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            rustls_backend(),
            connector,
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11, Version::HTTP_2],
        );
        let result = c.execute(templated_uri::BaseUri::from_static("http://example.com")).await;
        let Err(err) = result else {
            panic!("connector error should propagate");
        };
        assert!(err.to_string().contains("fail-rustls"), "got: {err}");
    }

    #[test]
    fn handle_tls_error_wraps_with_connect_label() {
        let inner: Box<dyn std::error::Error + Send + Sync> = Box::new(std::io::Error::new(std::io::ErrorKind::TimedOut, "boom"));
        let err = handle_tls_error(inner);
        assert!(err.to_string().contains("boom"), "got: {err}");
    }
}
