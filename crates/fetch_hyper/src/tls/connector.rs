// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The internal `TLS` connector, dispatching to the configured backend.

use std::marker::PhantomData;
use std::pin::Pin;
#[cfg(any(feature = "rustls", test))]
use std::sync::Arc;

use fetch_options::RequestFilter;
use fetch_tls::TlsBackend;
use http::Version;
use templated_uri::BaseUri;
#[cfg(any(feature = "rustls", feature = "native-tls", test))]
use tower::Service as _;

#[cfg(any(feature = "rustls", test))]
use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
#[cfg(any(feature = "native-tls", test))]
use crate::tls::native_tls::NativeTlsConnector;
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

impl<C, S> layered::Service<BaseUri> for TlsConnector<C, S>
where
    C: Connect<S>,
    // The TLS connectors require the wrapped transport stream to be `Unpin`,
    // even though `HyperIo` itself does not (the erased `Pin<Box<dyn HyperIo>>`
    // output remains usable for non-Unpin implementations).
    S: HyperIo + Unpin,
{
    type Out = http_extensions::Result<Pin<Box<dyn HyperIo>>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        match self {
            #[cfg(any(feature = "rustls", test))]
            Self::Rustls(c, _) => {
                let mut c = c.clone();
                // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "hyper-rustls delegates readiness to HyperConnectorAdapter, which always returns Ready(Ok(()))")]
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::pin(s) as Pin<Box<dyn HyperIo>>)
                    .map_err(handle_tls_error)
            }
            #[cfg(any(feature = "native-tls", test))]
            Self::NativeTls(c) => {
                let mut c = c.clone();
                // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "NativeTlsConnector delegates readiness to HyperConnectorAdapter, which always returns Ready(Ok(()))")]
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

    #[test]
    #[cfg_attr(miri, ignore = "native tests cover rustls configuration through native cryptography")]
    fn gamma_survivor_preconfigured_rustls_alpn_is_cleared_before_wrapping() {
        let TlsBackend::Rustls(config) = rustls_backend() else {
            panic!("rustls fixture must use rustls");
        };
        let mut config = (*config).clone();
        config.alpn_protocols = vec![b"caller-protocol".to_vec()];
        let _ = build_rustls_connector(
            config,
            fake_connector(),
            &RequestFilter::HttpAndHttps,
            &[Version::HTTP_11, Version::HTTP_2],
        );
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

    #[derive(Clone, Debug)]
    struct CapturingConnector {
        inner: FakeConnector,
        written: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl layered::Service<templated_uri::BaseUri> for CapturingConnector {
        type Out = http_extensions::Result<CapturingStream>;

        async fn execute(&self, input: templated_uri::BaseUri) -> Self::Out {
            Ok(CapturingStream {
                inner: self.inner.execute(input).await?,
                written: std::sync::Arc::clone(&self.written),
            })
        }
    }

    #[derive(Debug)]
    struct CapturingStream {
        inner: FakeStream,
        written: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl hyper::rt::Read for CapturingStream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: hyper::rt::ReadBufCursor<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
        }
    }

    impl hyper::rt::Write for CapturingStream {
        fn poll_write(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
            let this = self.get_mut();
            let result = Pin::new(&mut this.inner).poll_write(cx, buf);
            if let std::task::Poll::Ready(Ok(count)) = result {
                this.written.lock().unwrap().extend_from_slice(&buf[..count]);
            }
            result
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_flush(cx)
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
        }
    }

    impl hyper_util::client::legacy::connect::Connection for CapturingStream {
        fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
            self.inner.connected()
        }
    }

    fn client_hello_alpn(record: &[u8]) -> Vec<Vec<u8>> {
        fn take<'a>(bytes: &mut &'a [u8], length: usize) -> &'a [u8] {
            let (head, tail) = bytes.split_at(length);
            *bytes = tail;
            head
        }

        fn u16_length(bytes: &mut &[u8]) -> usize {
            let value = take(bytes, 2);
            usize::from(u16::from_be_bytes([value[0], value[1]]))
        }

        assert_eq!(record[0], 22, "the fake must capture a TLS handshake record");
        let mut payload = &record[5..];
        assert_eq!(take(&mut payload, 1), [1], "the handshake must be ClientHello");
        take(&mut payload, 3 + 2 + 32);
        let session_id_length = usize::from(take(&mut payload, 1)[0]);
        take(&mut payload, session_id_length);
        let cipher_length = u16_length(&mut payload);
        take(&mut payload, cipher_length);
        let compression_length = usize::from(take(&mut payload, 1)[0]);
        take(&mut payload, compression_length);
        let extensions_length = u16_length(&mut payload);
        let mut extensions = take(&mut payload, extensions_length);
        while !extensions.is_empty() {
            let kind = u16_length(&mut extensions);
            let length = u16_length(&mut extensions);
            let mut extension = take(&mut extensions, length);
            if kind == 16 {
                let protocol_length = u16_length(&mut extension);
                assert_eq!(extension.len(), protocol_length);
                let mut protocols = Vec::new();
                while !extension.is_empty() {
                    let length = usize::from(take(&mut extension, 1)[0]);
                    protocols.push(take(&mut extension, length).to_vec());
                }
                return protocols;
            }
        }
        Vec::new()
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "native tests cover ClientHello negotiation through native cryptography")]
    async fn gamma_survivor_rustls_client_hello_alpn_matches_supported_versions() {
        let cases = [
            (vec![Version::HTTP_11, Version::HTTP_2], vec![b"h2".to_vec(), b"http/1.1".to_vec()]),
            (vec![Version::HTTP_10, Version::HTTP_2], vec![b"h2".to_vec(), b"http/1.1".to_vec()]),
            (vec![Version::HTTP_2], vec![b"h2".to_vec()]),
            (vec![Version::HTTP_11], vec![]),
            (vec![Version::HTTP_10], vec![]),
        ];
        for (versions, expected) in cases {
            let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let connector = CapturingConnector {
                inner: fake_connector(),
                written: std::sync::Arc::clone(&written),
            };
            let tls = TlsConnector::new(rustls_backend(), connector, RequestFilter::Https, &versions);
            assert!(
                tls.execute(templated_uri::BaseUri::from_static("https://example.invalid"))
                    .await
                    .is_err()
            );
            assert_eq!(client_hello_alpn(&written.lock().unwrap()), expected, "{versions:?}");
        }
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
