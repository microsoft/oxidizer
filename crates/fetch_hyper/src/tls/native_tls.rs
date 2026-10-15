// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform native TLS connector implementation.

use std::pin::Pin;
use std::task::{Context, Poll};

use fetch_options::RequestFilter;
use http::Uri;
use hyper_tls::MaybeHttpsStream;
use hyper_util::rt::TokioIo;

use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::{Connect, HyperIo};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

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
    type Response = MaybeHttpsStream<S>;
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
            Some("http") => return error("https required but URI was not https"),
            Some(scheme) => return error(format!("unsupported URI scheme: {scheme}")),
            None => return error("URI scheme is required"),
        };

        let server_name = server_name(&self.backend, &req);
        let connecting = self.http.call(req);
        let tls = self.tls.clone();

        Box::pin(async move {
            let stream = connecting.await.map_err(|error| Box::new(error) as BoxError)?;
            if is_https {
                connect_https(tls, &server_name, stream).await
            } else {
                Ok(MaybeHttpsStream::Http(stream))
            }
        })
    }
}

// The scripted-peer test covers forwarding the selected server name into a
// real ClientHello. A successful platform certificate-validation path remains
// out of scope because it would require platform-specific trust-store changes.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
async fn connect_https<S: HyperIo + Unpin>(
    tls: tokio_native_tls::TlsConnector,
    server_name: &str,
    stream: S,
) -> Result<MaybeHttpsStream<S>, BoxError> {
    let stream = tls.connect(server_name, TokioIo::new(stream)).await?;
    Ok(MaybeHttpsStream::Https(TokioIo::new(stream)))
}

fn error<S: HyperIo>(message: impl Into<BoxError>) -> Pin<Box<dyn Future<Output = Result<MaybeHttpsStream<S>, BoxError>> + Send>> {
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
    use std::io::{Read as _, Write as _};
    use std::net::{SocketAddr, TcpListener as StdTcpListener};
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use http::Version;
    use layered::Service as _;
    use templated_uri::BaseUri;
    use tick::Clock;
    use tokio::net::TcpStream;
    use tower::Service as _;

    use super::*;
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

        assert!(error.to_string().contains("unsupported URI scheme: ftp"), "got: {error}");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn http_is_rejected_when_https_is_required() {
        let mut connector = NativeTlsConnector::new(native_tls_backend_config_with_override(), fake_connector(), &RequestFilter::Https);

        let Err(error) = connector.call(Uri::from_static("http://example.com")).await else {
            panic!("plain HTTP should be rejected");
        };

        assert!(error.to_string().contains("https required"), "got: {error}");
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

        assert!(error.to_string().contains("URI scheme is required"), "got: {error}");
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

        connector
            .call(request)
            .await
            .expect_err("scripted TLS peer closes after reading ClientHello");

        assert_eq!(peer.join().unwrap(), expected_server_name);
        assert_eq!(destination.lock().unwrap().as_ref().unwrap().authority(), &request_authority);
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
