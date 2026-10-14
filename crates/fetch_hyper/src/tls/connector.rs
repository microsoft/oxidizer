// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The internal `TLS` connector, dispatching to the configured backend.

use std::marker::PhantomData;
use std::pin::Pin;
#[cfg(any(feature = "rustls", test))]
use std::sync::Arc;
#[cfg(any(feature = "native-tls", test))]
use std::task::{Context, Poll};

use fetch_options::RequestFilter;
use fetch_tls::TlsBackend;
#[cfg(any(feature = "native-tls", test))]
use http::Uri;
use http::Version;
#[cfg(any(feature = "native-tls", test))]
use hyper_tls::MaybeHttpsStream;
#[cfg(any(feature = "native-tls", test))]
use hyper_util::rt::TokioIo;
use templated_uri::BaseUri;
#[cfg(any(feature = "rustls", feature = "native-tls", test))]
use tower::Service as _;

#[cfg(any(feature = "rustls", feature = "native-tls", test))]
use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::{Connect, HyperIo};

/// Wraps the `TLS` connector, dispatching to the active backend at runtime.
pub(crate) enum TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    #[cfg(any(feature = "rustls", test))]
    Rustls(hyper_rustls::HttpsConnector<HyperConnectorAdapter<C, S>>, PhantomData<fn() -> S>),

    #[cfg(any(feature = "native-tls", test))]
    NativeTls(NativeTlsConnector<C, S>),

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
            Self::NativeTls(c) => Self::NativeTls(c.clone()),
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
                build_rustls_connector(Arc::unwrap_or_clone(config), connector, &request_filter, supported_versions),
                PhantomData,
            ),
            #[cfg(any(feature = "native-tls", test))]
            TlsBackend::NativeTls(native) => Self::NativeTls(NativeTlsConnector::new(native, connector, &request_filter)),

            // When `fetch_hyper` is built without any TLS feature but feature
            // unification (e.g. during `cargo test --doc` across the workspace)
            // still enables variants on `fetch_tls::TlsBackend`, the match
            // arms above are cfg'd out. This unreachable arm keeps the match
            // exhaustive in that configuration without affecting normal builds.
            #[cfg(not(any(feature = "rustls", feature = "native-tls", test)))]
            _ => no_tls_backend_unreachable(),
        }
    }
}

#[cfg(not(any(feature = "rustls", feature = "native-tls", test)))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cold]
fn no_tls_backend_unreachable() -> ! {
    unreachable!("`TlsBackend` variants cannot be constructed when no TLS feature is enabled in `fetch_hyper`")
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
    request_filter: &RequestFilter,
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
type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[cfg(any(feature = "native-tls", test))]
pub(crate) struct NativeTlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    force_https: bool,
    http: HyperConnectorAdapter<C, S>,
    tls: tokio_native_tls::TlsConnector,
    backend: fetch_tls::NativeTlsBackend,
}

#[cfg(any(feature = "native-tls", test))]
impl<C, S> NativeTlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    fn new(backend: fetch_tls::NativeTlsBackend, connector: C, request_filter: &RequestFilter) -> Self {
        Self {
            force_https: matches!(request_filter, RequestFilter::Https),
            http: HyperConnectorAdapter::new(connector),
            tls: tokio_native_tls::TlsConnector::from(backend.connector().clone()),
            backend,
        }
    }
}

#[cfg(any(feature = "native-tls", test))]
impl<C, S> Clone for NativeTlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    fn clone(&self) -> Self {
        Self {
            force_https: self.force_https,
            http: self.http.clone(),
            tls: self.tls.clone(),
            backend: self.backend.clone(),
        }
    }
}

#[cfg(any(feature = "native-tls", test))]
impl<C, S> tower::Service<Uri> for NativeTlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    type Response = MaybeHttpsStream<S>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.http.poll_ready(cx).map(|result| result.map_err(Into::into))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let is_https = req.scheme_str() == Some("https");
        if !is_https && self.force_https {
            return Box::pin(async { Err::<MaybeHttpsStream<S>, BoxError>("https required but URI was not https".into()) });
        }

        let server_name = native_tls_server_name(&self.backend, &req);
        let connecting = self.http.call(req);
        let tls = self.tls.clone();

        Box::pin(async move {
            let stream = connecting.await.map_err(|error| Box::new(error) as BoxError)?;
            if is_https {
                let stream = tls.connect(&server_name, TokioIo::new(stream)).await?;
                Ok(MaybeHttpsStream::Https(TokioIo::new(stream)))
            } else {
                Ok(MaybeHttpsStream::Http(stream))
            }
        })
    }
}

#[cfg(any(feature = "native-tls", test))]
fn native_tls_server_name(backend: &fetch_tls::NativeTlsBackend, destination: &Uri) -> String {
    let host = destination.host().unwrap_or("").trim_matches(|c| c == '[' || c == ']');
    destination
        .authority()
        .and_then(|authority| backend.server_name_for(authority))
        .unwrap_or(host)
        .to_owned()
}

impl<C, S> layered::Service<BaseUri> for TlsConnector<C, S>
where
    C: Connect<S>,
    // `hyper-rustls`/`hyper-tls` only implement `Service` for their
    // `HttpsConnector` when the wrapped transport stream is `Unpin`, so the
    // inner stream type must be `Unpin` here even though `HyperIo` itself no
    // longer requires it (the erased `Pin<Box<dyn HyperIo>>` output does not).
    S: HyperIo + Unpin,
{
    type Out = http_extensions::Result<Pin<Box<dyn HyperIo>>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        match self {
            #[cfg(any(feature = "rustls", test))]
            Self::Rustls(c, _) => {
                let mut c = c.clone();
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::pin(s) as Pin<Box<dyn HyperIo>>)
                    .map_err(handle_tls_error)
            }
            #[cfg(any(feature = "native-tls", test))]
            Self::NativeTls(c) => {
                let mut c = c.clone();
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::pin(s) as Pin<Box<dyn HyperIo>>)
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
        native_tls::TlsConnector::new().unwrap().into()
    }

    fn native_tls_backend_config_with_override() -> fetch_tls::NativeTlsBackend {
        let options = fetch_tls::TlsOptions::builder_native_tls()
            .server_name_override("example.com", "expected.example.test")
            .build();
        let backend = fetch_tls::TlsBackendBuilder::new().build_backend(options).unwrap();
        let TlsBackend::NativeTls(backend) = backend else {
            panic!("expected native TLS backend");
        };
        backend
    }

    fn native_tls_backend_with_override() -> TlsBackend {
        TlsBackend::NativeTls(native_tls_backend_config_with_override())
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
        assert!(matches!(c, TlsConnector::NativeTls(_)));
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
        assert!(matches!(c, TlsConnector::NativeTls(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_with_native_tls_server_name_override_creates_native_variant() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend_with_override(),
            fake_connector(),
            RequestFilter::Https,
            &[Version::HTTP_11],
        );
        assert!(matches!(c, TlsConnector::NativeTls(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn native_tls_server_name_uses_override_without_changing_destination() {
        let destination = Uri::from_static("https://example.com:8443/");
        let server_name = native_tls_server_name(&native_tls_backend_config_with_override(), &destination);

        assert_eq!(server_name, "expected.example.test");
        assert_eq!(destination.authority().unwrap().as_str(), "example.com:8443");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn native_tls_server_name_falls_back_to_destination_host() {
        let destination = Uri::from_static("https://other.example.com:8443/");
        let server_name = native_tls_server_name(&native_tls_backend_config_with_override(), &destination);

        assert_eq!(server_name, "other.example.com");
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
        assert!(matches!(c, TlsConnector::NativeTls(_)));
        assert!(matches!(c2, TlsConnector::NativeTls(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn clone_native_tls_variant_with_override() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend_with_override(),
            fake_connector(),
            RequestFilter::Https,
            &[Version::HTTP_11],
        );
        let c2 = c.clone();
        assert!(matches!(c, TlsConnector::NativeTls(_)));
        assert!(matches!(c2, TlsConnector::NativeTls(_)));
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
    async fn execute_native_tls_override_with_plain_http_returns_stream() {
        let c: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend_with_override(),
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
