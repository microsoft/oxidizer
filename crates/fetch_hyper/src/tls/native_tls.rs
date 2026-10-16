// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform native TLS connector implementation.

use std::io::IoSlice;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use fetch_options::RequestFilter;
use http::Uri;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use seatbelt::RecoveryInfo;

use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::{Connect, HyperIo};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type RecordedErrorKind = Arc<Mutex<Option<std::io::ErrorKind>>>;

#[derive(Debug)]
struct HttpsRequired;

impl std::fmt::Display for HttpsRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("https required but URI was not https")
    }
}

impl std::error::Error for HttpsRequired {}

#[derive(Debug)]
struct UnsupportedScheme(String);

impl std::fmt::Display for UnsupportedScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported URI scheme: {}", self.0)
    }
}

impl std::error::Error for UnsupportedScheme {}

#[derive(Debug)]
struct MissingScheme;

impl std::fmt::Display for MissingScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("URI scheme is required")
    }
}

impl std::error::Error for MissingScheme {}

#[cfg_attr(
    windows,
    expect(
        clippy::large_enum_variant,
        reason = "boxing the established connection adds a heap allocation to every successful native TLS connection"
    )
)]
pub(crate) enum NativeTlsStream<S> {
    Http(S),
    Https(TokioIo<tokio_native_tls::TlsStream<TokioIo<RecordingIo<S>>>>),
}

// Pure I/O forwarding preserves the behavior of the wrapped HTTP/TLS stream;
// mutating these methods only creates equivalent behavior or artificial hangs.
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl<S: Read + Write + Unpin> Read for NativeTlsStream<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Https(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl<S: Read + Write + Unpin> Write for NativeTlsStream<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Https(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Self::Https(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Http(stream) => stream.is_write_vectored(),
            Self::Https(stream) => stream.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_flush(cx),
            Self::Https(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Https(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

impl<S: HyperIo + Unpin> Connection for NativeTlsStream<S> {
    fn connected(&self) -> Connected {
        match self {
            Self::Http(stream) => stream.connected(),
            Self::Https(stream) => {
                let connected = stream.inner().get_ref().get_ref().get_ref().inner().inner.connected();
                if negotiated_h2(stream.inner().get_ref()) {
                    connected.negotiated_h2()
                } else {
                    connected
                }
            }
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Native TLS owns ALPN negotiation; this only maps its reported protocol into hyper metadata.
#[cfg_attr(coverage_nightly, coverage(off))]
fn negotiated_h2<S: std::io::Read + std::io::Write>(stream: &native_tls::TlsStream<S>) -> bool {
    stream.negotiated_alpn().unwrap_or(None).is_some_and(|protocol| protocol == b"h2")
}

pub(crate) struct RecordingIo<S> {
    inner: S,
    error_kind: RecordedErrorKind,
}

// Some native TLS backends, notably Schannel, do not retain the underlying
// transport error in the returned TLS error chain. Recording it at the I/O
// boundary preserves transient error classification. A handshake failure
// without a recorded I/O error is treated as nonretryable, as for TLS alerts
// and certificate or protocol validation failures.

impl<S: Read + Unpin> Read for RecordingIo<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        record_error_kind(&self.error_kind, &result);
        result
    }
}

impl<S: Write + Unpin> Write for RecordingIo<S> {
    #[cfg_attr(test, mutants::skip)] // Returning artificial zero-progress writes makes native-tls spin without yielding.
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        record_error_kind(&self.error_kind, &result);
        result
    }

    #[cfg_attr(test, mutants::skip)] // Pure forwarding; native TLS handshakes use poll_write.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        record_error_kind(&self.error_kind, &result);
        result
    }

    #[cfg_attr(test, mutants::skip)] // Pure capability forwarding.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    #[cfg_attr(test, mutants::skip)] // Pure forwarding; the handshake's transient I/O path is covered through poll_read.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        record_error_kind(&self.error_kind, &result);
        result
    }

    #[cfg_attr(test, mutants::skip)] // Pure forwarding after connection establishment.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_shutdown(cx);
        record_error_kind(&self.error_kind, &result);
        result
    }
}

#[cfg_attr(test, mutants::skip)] // Pure connection-metadata forwarding.
#[cfg_attr(coverage_nightly, coverage(off))]
impl<S: Connection> Connection for RecordingIo<S> {
    fn connected(&self) -> Connected {
        self.inner.connected()
    }
}

fn record_error_kind<T>(recorded: &RecordedErrorKind, result: &Poll<std::io::Result<T>>) {
    if let Poll::Ready(Err(error)) = result {
        *recorded
            .lock()
            .expect("handshake error recorder mutex must not be poisoned because recording does not execute user code") =
            Some(error.kind());
    }
}

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

impl<C, S> NativeTlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    pub(super) fn new(backend: fetch_tls::NativeTlsBackend, connector: C, request_filter: &RequestFilter) -> Self {
        Self {
            force_https: matches!(request_filter, RequestFilter::Https),
            http: HyperConnectorAdapter::new(connector),
            tls: tokio_native_tls::TlsConnector::from(backend.connector().clone()),
            backend,
        }
    }
}

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

impl<C, S> tower::Service<Uri> for NativeTlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    type Response = NativeTlsStream<S>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    #[cfg_attr(test, mutants::skip)] // Pure readiness delegation; replacing Ready(Ok(())) is observationally equivalent for the adapter.
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.http.poll_ready(cx).map(|result| result.map_err(Into::into))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let is_https = match req.scheme_str() {
            Some("https") => true,
            Some("http") if !self.force_https => false,
            Some("http") => return error(HttpsRequired),
            Some(scheme) => return error(UnsupportedScheme(scheme.to_owned())),
            None => return error(MissingScheme),
        };

        let server_name = server_name(&self.backend, &req);
        let connecting = self.http.call(req);
        let tls = self.tls.clone();

        Box::pin(async move {
            let stream = connecting.await.map_err(|error| Box::new(error) as BoxError)?;
            if is_https {
                connect_https(tls, &server_name, stream).await
            } else {
                Ok(NativeTlsStream::Http(stream))
            }
        })
    }
}

// The scripted-peer test covers forwarding the selected server name into a
// real ClientHello. A successful platform certificate-validation path remains
// out of scope because it would require platform-specific trust-store changes.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn connect_https<S: HyperIo + Unpin>(
    tls: tokio_native_tls::TlsConnector,
    server_name: &str,
    stream: S,
) -> Result<NativeTlsStream<S>, BoxError> {
    let error_kind = Arc::new(Mutex::new(None));
    let recording = RecordingIo {
        inner: stream,
        error_kind: Arc::clone(&error_kind),
    };
    match tls.connect(server_name, TokioIo::new(recording)).await {
        Ok(stream) => Ok(wrap_https_stream(stream)),
        Err(error) => {
            let recovery = error_kind
                .lock()
                .expect("handshake error recorder mutex must not be poisoned because recording does not execute user code")
                .map_or_else(RecoveryInfo::never, RecoveryInfo::from);
            Err(Box::new(http_extensions::HttpError::other(
                error,
                recovery,
                crate::error_labels::LABEL_CONNECT,
            )))
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Type-only adaptation after a successful platform TLS handshake.
#[cfg_attr(coverage_nightly, coverage(off))]
fn wrap_https_stream<S>(stream: tokio_native_tls::TlsStream<TokioIo<RecordingIo<S>>>) -> NativeTlsStream<S> {
    NativeTlsStream::Https(TokioIo::new(stream))
}

fn error<S: HyperIo>(message: impl Into<BoxError>) -> Pin<Box<dyn Future<Output = Result<NativeTlsStream<S>, BoxError>> + Send>> {
    let error = message.into();
    Box::pin(async move { Err(error) })
}

fn server_name(backend: &fetch_tls::NativeTlsBackend, destination: &Uri) -> String {
    let host = destination.host().unwrap_or("").trim_start_matches('[').trim_end_matches(']');
    destination
        .authority()
        .and_then(|authority| backend.server_name_for(authority))
        .unwrap_or(host)
        .to_owned()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error as _;
    use std::io::{Read as _, Write as _};
    use std::net::{SocketAddr, TcpListener as StdTcpListener};

    use anyspawn::Spawner;
    use bytes::Bytes;
    use fetch_options::TransportOptions;
    use http::{Method, StatusCode, Version};
    use http_extensions::{HttpBodyBuilder, HttpRequestBuilder};
    use layered::Service as _;
    use rcgen::{CertificateParams, KeyPair, PKCS_RSA_SHA256};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use seatbelt::{Recovery, RecoveryKind};
    use templated_uri::BaseUri;
    use tick::Clock;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tower::Service as _;

    use super::*;
    use crate::HyperTransportBuilder;
    use crate::testing::{FakeConnector, FakeStream, TestError};
    use crate::tls::connector::TlsConnector;

    fn native_tls_backend() -> fetch_tls::TlsBackend {
        native_tls::TlsConnector::new().unwrap().into()
    }

    fn native_tls_backend_config_with_override() -> fetch_tls::NativeTlsBackend {
        let options = fetch_tls::TlsOptions::builder_native_tls()
            .server_name_override("example.com", "expected.example.test")
            .build();
        let backend = fetch_tls::TlsBackendBuilder::new().build_backend(options).unwrap();
        let fetch_tls::TlsBackend::NativeTls(backend) = backend else {
            panic!("expected native TLS backend");
        };
        backend
    }

    fn native_tls_backend_with_override() -> fetch_tls::TlsBackend {
        fetch_tls::TlsBackend::NativeTls(native_tls_backend_config_with_override())
    }

    fn fake_connector() -> FakeConnector {
        FakeConnector::new_success(Bytes::new(), Clock::new_frozen())
    }

    struct LoopbackTls {
        address: SocketAddr,
        root: native_tls::Certificate,
        server_config: Arc<rustls::ServerConfig>,
    }

    impl LoopbackTls {
        async fn new(server_name: &str) -> (Self, TcpListener) {
            let leaf_key = KeyPair::generate_for(&PKCS_RSA_SHA256).unwrap();
            let leaf = CertificateParams::new([server_name.to_owned()])
                .unwrap()
                .self_signed(&leaf_key)
                .unwrap();
            let root = native_tls::Certificate::from_der(leaf.der()).unwrap();
            let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
            let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![leaf.der().clone()], private_key)
                .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            (
                Self {
                    address,
                    root,
                    server_config: Arc::new(server_config),
                },
                listener,
            )
        }

        fn backend(&self, endpoint: &str, server_name: &str, versions: &[Version]) -> fetch_tls::TlsBackend {
            let options = fetch_tls::TlsOptions::builder_native_tls()
                .root_certificate(self.root.clone())
                .server_name_override(endpoint, server_name)
                .supported_http_versions(versions)
                .build();
            fetch_tls::TlsBackendBuilder::new().build_backend(options).unwrap()
        }

        fn acceptor(&self, protocols: &[&str]) -> tokio_rustls::TlsAcceptor {
            let mut config = (*self.server_config).clone();
            config.alpn_protocols = protocols.iter().map(|protocol| protocol.as_bytes().to_vec()).collect();
            tokio_rustls::TlsAcceptor::from(Arc::new(config))
        }
    }

    struct ResetStream;

    impl hyper::rt::Read for ResetStream {
        fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
        }
    }

    impl Write for ResetStream {
        fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl Connection for ResetStream {
        fn connected(&self) -> Connected {
            Connected::new()
        }
    }

    struct WriteResetStream;

    impl hyper::rt::Read for WriteResetStream {
        fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }

    impl Write for WriteResetStream {
        fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl Connection for WriteResetStream {
        fn connected(&self) -> Connected {
            Connected::new()
        }
    }

    #[derive(Clone)]
    struct ResetConnector;

    impl layered::Service<BaseUri> for ResetConnector {
        type Out = http_extensions::Result<ResetStream>;

        async fn execute(&self, _input: BaseUri) -> Self::Out {
            Ok(ResetStream)
        }
    }

    #[derive(Clone)]
    struct WriteResetConnector;

    impl layered::Service<BaseUri> for WriteResetConnector {
        type Out = http_extensions::Result<WriteResetStream>;

        async fn execute(&self, _input: BaseUri) -> Self::Out {
            Ok(WriteResetStream)
        }
    }

    #[derive(Clone)]
    struct RecordingTcpConnector {
        address: SocketAddr,
        destination: Arc<Mutex<Option<BaseUri>>>,
    }

    impl layered::Service<BaseUri> for RecordingTcpConnector {
        type Out = http_extensions::Result<TokioIo<TcpStream>>;

        async fn execute(&self, input: BaseUri) -> Self::Out {
            *self.destination.lock().unwrap() = Some(input);
            Ok(TokioIo::new(TcpStream::connect(self.address).await?))
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_creates_native_variant() {
        let connector: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(native_tls_backend(), fake_connector(), RequestFilter::Https, &[Version::HTTP_11]);
        assert!(matches!(connector, TlsConnector::NativeTls(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn server_name_uses_override_without_changing_destination() {
        let destination = Uri::from_static("https://example.com:8443/");
        let server_name = server_name(&native_tls_backend_config_with_override(), &destination);

        assert_eq!(server_name, "expected.example.test");
        assert_eq!(destination.authority().unwrap().as_str(), "example.com:8443");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn server_name_falls_back_to_destination_host() {
        let destination = Uri::from_static("https://other.example.com:8443/");
        let server_name = server_name(&native_tls_backend_config_with_override(), &destination);

        assert_eq!(server_name, "other.example.com");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn clone_preserves_native_variant_with_override() {
        let connector: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend_with_override(),
            fake_connector(),
            RequestFilter::Https,
            &[Version::HTTP_11],
        );
        let clone = connector.clone();
        assert!(matches!(connector, TlsConnector::NativeTls(_)));
        assert!(matches!(clone, TlsConnector::NativeTls(_)));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn plain_http_returns_stream_when_allowed() {
        let connector: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend(),
            fake_connector(),
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11],
        );

        connector
            .execute(templated_uri::BaseUri::from_static("http://example.com"))
            .await
            .unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn unsupported_scheme_is_rejected() {
        let connector: TlsConnector<FakeConnector, FakeStream> = TlsConnector::new(
            native_tls_backend(),
            fake_connector(),
            RequestFilter::HttpAndHttps,
            &[Version::HTTP_11],
        );

        let Err(error) = connector.execute(templated_uri::BaseUri::from_static("ftp://example.com")).await else {
            panic!("unsupported scheme should be rejected");
        };

        let cause = error
            .source()
            .and_then(|source| source.downcast_ref::<UnsupportedScheme>())
            .expect("unsupported scheme cause should be preserved");
        assert_eq!(cause.0, "ftp");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn http_is_rejected_when_https_is_required() {
        let mut connector = NativeTlsConnector::new(native_tls_backend_config_with_override(), fake_connector(), &RequestFilter::Https);

        let Err(error) = connector.call(Uri::from_static("http://example.com")).await else {
            panic!("plain HTTP should be rejected");
        };

        assert!(error.downcast_ref::<HttpsRequired>().is_some(), "got: {error}");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn missing_scheme_is_rejected() {
        let mut connector = NativeTlsConnector::new(
            native_tls_backend_config_with_override(),
            fake_connector(),
            &RequestFilter::HttpAndHttps,
        );

        let Err(error) = connector.call(Uri::from_static("/relative")).await else {
            panic!("missing scheme should be rejected");
        };

        assert!(error.downcast_ref::<MissingScheme>().is_some(), "got: {error}");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn override_reaches_client_hello_without_changing_destination() {
        let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = format!("localhost:{}", address.port());
        let expected_server_name = "expected.example.test";
        let destination = Arc::new(Mutex::new(None));
        let tcp_connector = RecordingTcpConnector {
            address,
            destination: Arc::clone(&destination),
        };
        let options = fetch_tls::TlsOptions::builder_native_tls()
            .server_name_override(endpoint.as_str(), expected_server_name)
            .build();
        let backend = fetch_tls::TlsBackendBuilder::new().build_backend(options).unwrap();
        let fetch_tls::TlsBackend::NativeTls(backend) = backend else {
            panic!("expected native TLS backend");
        };
        let mut connector = NativeTlsConnector::new(backend, tcp_connector, &RequestFilter::Https);
        let request = Uri::builder()
            .scheme("https")
            .authority(endpoint.as_str())
            .path_and_query("/original-path/")
            .build()
            .unwrap();
        let request_authority = request.authority().unwrap().clone();

        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let server_name = read_client_hello_server_name(&mut stream);
            stream.write_all(&[21, 3, 3, 0, 2, 2, 40]).unwrap();
            server_name
        });

        let Err(error) = tokio::time::timeout(std::time::Duration::from_secs(5), connector.call(request))
            .await
            .expect("native TLS handshake must not stall")
        else {
            panic!("scripted TLS peer should fail after reading ClientHello");
        };

        assert_eq!(peer.join().unwrap(), expected_server_name);
        assert_eq!(destination.lock().unwrap().as_ref().unwrap().authority(), &request_authority);
        assert_eq!(
            error.downcast_ref::<http_extensions::HttpError>().unwrap().recovery().kind(),
            RecoveryKind::Never
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn handshake_connection_reset_is_retryable() {
        let fetch_tls::TlsBackend::NativeTls(backend) = native_tls_backend() else {
            panic!("expected native TLS backend");
        };
        let mut connector = NativeTlsConnector::new(backend, ResetConnector, &RequestFilter::Https);

        let Err(error) = connector.call(Uri::from_static("https://example.com/")).await else {
            panic!("connection reset should fail the handshake");
        };

        assert_eq!(
            error.downcast_ref::<http_extensions::HttpError>().unwrap().recovery().kind(),
            RecoveryKind::Retry
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn handshake_write_reset_is_retryable_without_stalling() {
        let fetch_tls::TlsBackend::NativeTls(backend) = native_tls_backend() else {
            panic!("expected native TLS backend");
        };
        let mut connector = NativeTlsConnector::new(backend, WriteResetConnector, &RequestFilter::Https);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            connector.call(Uri::from_static("https://example.com/")),
        )
        .await
        .expect("handshake write errors must not stall");
        let Err(error) = result else {
            panic!("connection reset should fail the handshake");
        };

        assert_eq!(
            error.downcast_ref::<http_extensions::HttpError>().unwrap().recovery().kind(),
            RecoveryKind::Retry
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn matching_override_succeeds_and_preserves_http_authority() {
        let expected_server_name = "expected.example.test";
        let (server, listener) = LoopbackTls::new(expected_server_name).await;
        let endpoint = format!("localhost:{}", server.address.port());
        let observed_host = Arc::new(Mutex::new(None));
        let host_capture = Arc::clone(&observed_host);
        let acceptor = server.acceptor(&["http/1.1"]);
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0);
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8(request).unwrap();
            let host = request
                .lines()
                .find_map(|line| line.strip_prefix("host: ").or_else(|| line.strip_prefix("Host: ")))
                .unwrap()
                .trim()
                .to_owned();
            *host_capture.lock().unwrap() = Some(host);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let destination = Arc::new(Mutex::new(None));
        let connector = RecordingTcpConnector {
            address: server.address,
            destination: Arc::clone(&destination),
        };
        let mut transport_options = TransportOptions::default();
        transport_options.supported_http_versions = vec![Version::HTTP_11];
        let handler = HyperTransportBuilder::new(connector, Spawner::new_tokio(), Clock::new_tokio(), transport_options)
            .body_builder(HttpBodyBuilder::new_fake())
            .build(server.backend(&endpoint, expected_server_name, &[Version::HTTP_11]));
        let body_builder = HttpBodyBuilder::new_fake();
        let request = HttpRequestBuilder::new(&body_builder)
            .method(Method::GET)
            .uri(format!("https://{endpoint}/hello"))
            .build()
            .unwrap();

        let response = tokio::time::timeout(std::time::Duration::from_secs(5), handler.execute(request))
            .await
            .expect("native TLS request must not stall")
            .unwrap();
        peer.await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(observed_host.lock().unwrap().as_deref(), Some(endpoint.as_str()));
        assert_eq!(destination.lock().unwrap().as_ref().unwrap().authority().as_str(), endpoint);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn mismatched_override_fails_certificate_validation() {
        let (server, listener) = LoopbackTls::new("expected.example.test").await;
        let endpoint = format!("localhost:{}", server.address.port());
        let acceptor = server.acceptor(&["http/1.1"]);
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = acceptor.accept(stream).await;
        });
        let connector = RecordingTcpConnector {
            address: server.address,
            destination: Arc::new(Mutex::new(None)),
        };
        let fetch_tls::TlsBackend::NativeTls(backend) = server.backend(&endpoint, "wrong.example.test", &[Version::HTTP_11]) else {
            panic!("expected native TLS backend");
        };
        let mut connector = NativeTlsConnector::new(backend, connector, &RequestFilter::Https);

        let Err(error) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connector.call(Uri::from_maybe_shared(format!("https://{endpoint}/")).unwrap()),
        )
        .await
        .expect("native TLS handshake must not stall") else {
            panic!("mismatched override must fail certificate validation");
        };
        peer.await.unwrap();

        assert_eq!(
            error.downcast_ref::<http_extensions::HttpError>().unwrap().recovery().kind(),
            RecoveryKind::Never
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn successful_h2_negotiation_sets_connection_metadata() {
        let expected_server_name = "expected.example.test";
        let (server, listener) = LoopbackTls::new(expected_server_name).await;
        let endpoint = format!("localhost:{}", server.address.port());
        let acceptor = server.acceptor(&["h2"]);
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _stream = acceptor.accept(stream).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        let connector = RecordingTcpConnector {
            address: server.address,
            destination: Arc::new(Mutex::new(None)),
        };
        let fetch_tls::TlsBackend::NativeTls(backend) = server.backend(&endpoint, expected_server_name, &[Version::HTTP_2]) else {
            panic!("expected native TLS backend");
        };
        let mut connector = NativeTlsConnector::new(backend, connector, &RequestFilter::Https);

        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connector.call(Uri::from_maybe_shared(format!("https://{endpoint}/")).unwrap()),
        )
        .await
        .expect("native TLS handshake must not stall")
        .unwrap();

        assert!(stream.connected().is_negotiated_h2());
        peer.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_error_propagates() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_connect_failure(TestError::new("fail"), clock);
        let connector: TlsConnector<FakeConnector, FakeStream> =
            TlsConnector::new(native_tls_backend(), connector, RequestFilter::HttpAndHttps, &[Version::HTTP_11]);

        let Err(error) = connector.execute(templated_uri::BaseUri::from_static("http://example.com")).await else {
            panic!("connector error should propagate");
        };

        assert!(error.to_string().contains("fail"), "got: {error}");
    }

    fn read_client_hello_server_name(stream: &mut std::net::TcpStream) -> String {
        let mut header = [0_u8; 5];
        stream.read_exact(&mut header).unwrap();
        let record_length = usize::from(u16::from_be_bytes([header[3], header[4]]));
        let mut bytes = Vec::with_capacity(record_length + header.len());
        bytes.extend_from_slice(&header);
        bytes.resize(record_length + header.len(), 0);
        stream.read_exact(&mut bytes[header.len()..]).unwrap();
        parse_client_hello_server_name(&bytes).expect("ClientHello should contain an SNI server name")
    }

    fn parse_client_hello_server_name(records: &[u8]) -> Option<String> {
        let mut handshake = Vec::new();
        let mut record_offset = 0;
        while records.len().checked_sub(record_offset)? >= 5 {
            let record_length = usize::from(u16::from_be_bytes([records[record_offset + 3], records[record_offset + 4]]));
            let payload_start = record_offset + 5;
            let payload_end = payload_start.checked_add(record_length)?;
            if records.len() < payload_end {
                return None;
            }
            if records[record_offset] == 22 {
                handshake.extend_from_slice(&records[payload_start..payload_end]);
            }
            record_offset = payload_end;
        }

        if handshake.len() < 4 || handshake[0] != 1 {
            return None;
        }
        let handshake_length = (usize::from(handshake[1]) << 16) | (usize::from(handshake[2]) << 8) | usize::from(handshake[3]);
        if handshake.len() < handshake_length + 4 {
            return None;
        }

        parse_server_name_extension(&handshake[4..handshake_length + 4])
    }

    fn parse_server_name_extension(client_hello: &[u8]) -> Option<String> {
        let mut offset = 2 + 32;
        offset = skip_u8_length_prefixed(client_hello, offset)?;
        offset = skip_u16_length_prefixed(client_hello, offset)?;
        offset = skip_u8_length_prefixed(client_hello, offset)?;
        let extensions_length = read_u16(client_hello, offset)?;
        offset += 2;
        let all_extensions_end = offset.checked_add(extensions_length)?;
        if client_hello.len() < all_extensions_end {
            return None;
        }

        while offset < all_extensions_end {
            let extension_type = read_u16(client_hello, offset)?;
            let current_length = read_u16(client_hello, offset + 2)?;
            offset += 4;
            let current_end = offset.checked_add(current_length)?;
            if current_end > all_extensions_end {
                return None;
            }
            if extension_type == 0 {
                return parse_server_name_list(&client_hello[offset..current_end]);
            }
            offset = current_end;
        }
        None
    }

    fn parse_server_name_list(extension: &[u8]) -> Option<String> {
        let list_length = read_u16(extension, 0)?;
        let mut offset = 2_usize;
        let list_end = offset.checked_add(list_length)?;
        if extension.len() < list_end {
            return None;
        }

        while offset < list_end {
            let name_type = *extension.get(offset)?;
            let name_length = read_u16(extension, offset + 1)?;
            offset += 3;
            let name_end = offset.checked_add(name_length)?;
            if name_end > list_end {
                return None;
            }
            if name_type == 0 {
                return std::str::from_utf8(&extension[offset..name_end]).ok().map(str::to_owned);
            }
            offset = name_end;
        }
        None
    }

    fn skip_u8_length_prefixed(bytes: &[u8], offset: usize) -> Option<usize> {
        let length = usize::from(*bytes.get(offset)?);
        offset.checked_add(1)?.checked_add(length).filter(|end| *end <= bytes.len())
    }

    fn skip_u16_length_prefixed(bytes: &[u8], offset: usize) -> Option<usize> {
        let length = read_u16(bytes, offset)?;
        offset.checked_add(2)?.checked_add(length).filter(|end| *end <= bytes.len())
    }

    fn read_u16(bytes: &[u8], offset: usize) -> Option<usize> {
        Some(usize::from(u16::from_be_bytes([*bytes.get(offset)?, *bytes.get(offset + 1)?])))
    }
}
