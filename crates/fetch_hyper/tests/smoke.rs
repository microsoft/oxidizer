// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Smoke test verifying the builder produces a usable handler that can issue
//! a real HTTP request against a local mock server using the `native-tls`
//! backend and the `HttpAndHttps` request filter.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::time::Duration;

use anyspawn::Spawner;
use bytes::Bytes;
use fetch_hyper::HyperTransportBuilder;
use fetch_options::{RequestFilter, TransportOptions};
use fetch_tls::TlsBackend;
use http::{Method, StatusCode, Version};
use http_extensions::{HttpBodyBuilder, HttpRequestBuilder, Result};
use hyper_util::rt::TokioIo;
use layered::Service as _;
use ohno::ErrorExt;
use templated_uri::BaseUri;
use tick::{Clock, ClockControl};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone)]
struct TokioConnector;

impl layered::Service<BaseUri> for TokioConnector {
    type Out = Result<TokioIo<tokio::net::TcpStream>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        let stream = tokio::net::TcpStream::connect((input.authority().host(), input.effective_port().unwrap())).await?;
        Ok(TokioIo::new(stream))
    }
}

fn build_tls() -> TlsBackend {
    native_tls::TlsConnector::new().unwrap().into()
}

fn test_clock() -> Clock {
    ClockControl::new().auto_advance_timers(true).to_clock()
}

/// Builds [`TransportOptions`] with `HttpAndHttps`, a 5s connect timeout, and
/// the caller-supplied list of supported HTTP versions (empty `versions` keeps
/// the default).
fn options(versions: &[Version]) -> TransportOptions {
    let mut options = TransportOptions::default();
    options.request_filter = RequestFilter::HttpAndHttps;
    options.connect_timeout = Duration::from_secs(5);
    if !versions.is_empty() {
        options.supported_http_versions = versions.to_vec();
    }
    options
}

async fn serve(body: impl Into<Bytes>) -> MockServer {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/hello-world"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.into().to_vec()))
        .mount(&mock_server)
        .await;

    mock_server
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn real_http_request_succeeds() {
    let handler = HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(None), test_clock(), options(&[]))
        .body_builder(HttpBodyBuilder::new_fake())
        .build(build_tls());

    let server = serve(Bytes::from_static(b"Hello World!")).await;

    let body_builder = HttpBodyBuilder::new_fake();
    let request = HttpRequestBuilder::new(&body_builder)
        .method(Method::GET)
        .uri(server.uri() + "/hello-world")
        .build()
        .unwrap();

    let response = handler.execute(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[cfg_attr(miri, ignore = "TLS diagnostics use real loopback sockets and native cryptography")]
#[tokio::test]
async fn rustls_handshake_errors_keep_certificate_details_and_original_validation_errors() {
    use std::error::Error;
    use std::sync::Arc;

    use fetch_tls::{TlsBackendBuilder, TlsOptions};
    use rcgen::{CertificateParams, DnType, KeyPair};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use seatbelt::{Recovery, RecoveryKind};
    use tokio::net::TcpListener;

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    let mut peers = Vec::new();
    let mut endpoints = Vec::new();
    for name in ["first.example.test", "second.example.test"] {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new([name.to_owned()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        let certificate = params.self_signed(&key).unwrap();
        roots.add(certificate.der().clone()).unwrap();
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone()],
                PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        endpoints.push(address);
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
        peers.push(tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            acceptor.accept(stream).await.unwrap_err();
        }));
    }
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
        .build()
        .unwrap();
    let backend = TlsBackendBuilder::new()
        .configure_rustls(provider, verifier)
        .build_backend(TlsOptions::new_rustls())
        .unwrap();
    let handler = HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(None), Clock::new_tokio(), options(&[]))
        .body_builder(HttpBodyBuilder::new_fake())
        .build(backend);
    let request = |address| HttpRequestBuilder::new_fake().get(format!("https://{address}/")).build().unwrap();
    let results: [_; 2] = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(handler.execute(request(endpoints[0])), handler.execute(request(endpoints[1])))
    })
    .await
    .unwrap()
    .into();

    for ((result, address), (name, other_name)) in results.into_iter().zip(endpoints).zip([
        ("first.example.test", "second.example.test"),
        ("second.example.test", "first.example.test"),
    ]) {
        let error = result.unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&format!("remote_address={address}")), "{message}");
        assert!(message.contains("tls_validation_name=\"127.0.0.1\""), "{message}");
        assert!(message.contains("tls_sni_name=None"), "{message}");
        assert!(message.contains(&format!("peer_certificate_subject=\"CN={name}\"")), "{message}");
        assert!(message.contains(&format!("DNSName(\"{name}\")")), "{message}");
        assert!(message.contains("peer_certificate_sha256="), "{message}");
        assert!(!message.contains("peer_certificate_subject=unavailable"), "{message}");
        assert!(!message.contains(other_name), "{message}");
        assert_eq!(error.recovery().kind(), RecoveryKind::Never);

        let mut source: &(dyn Error + 'static) = &error;
        while !matches!(source.downcast_ref::<rustls::Error>(), Some(rustls::Error::InvalidCertificate(_))) {
            source = source.source().unwrap();
        }
    }
    for peer in peers {
        peer.await.unwrap();
    }
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn https_only_filter_rejects_http_request() {
    // No explicit `request_filter` override: defaults to `RequestFilter::Https`.
    let mut opts = TransportOptions::default();
    opts.connect_timeout = Duration::from_secs(5);
    let handler = HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(None), test_clock(), opts)
        .body_builder(HttpBodyBuilder::new_fake())
        .build(build_tls());

    let server = serve(Bytes::from_static(b"Hello World!")).await;

    let body_builder = HttpBodyBuilder::new_fake();
    let request = HttpRequestBuilder::new(&body_builder)
        .method(Method::GET)
        .uri(server.uri() + "/hello-world")
        .build()
        .unwrap();

    let error = handler.execute(request).await.unwrap_err();

    assert!(error.message().contains("https required but URI was not https"));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn http2_only_rejected_when_server_negotiates_http1() {
    // Wiremock speaks plain HTTP/1.1. The client advertises only HTTP/2 and
    // HTTP/3 (so HTTP/2 prior-knowledge is not auto-enabled), causing
    // post-connect protocol verification to reject the HTTP/1.1 connection.
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(None),
        test_clock(),
        options(&[Version::HTTP_2, Version::HTTP_3]),
    )
    .body_builder(HttpBodyBuilder::new_fake())
    .build(build_tls());

    let server = serve(Bytes::from_static(b"Hello World!")).await;

    let body_builder = HttpBodyBuilder::new_fake();
    let request = HttpRequestBuilder::new(&body_builder)
        .method(Method::GET)
        .uri(server.uri() + "/hello-world")
        .build()
        .unwrap();

    let error = handler.execute(request).await.unwrap_err();

    // Wiremock listens on a dynamic port, so the trailing `server: <uri>`
    // segment is non-deterministic; assert on the deterministic prefix.
    let message = error.message();
    let expected_prefix =
        "the connection was established with unsupported HTTP version: HTTP/1.1, supported versions are: [HTTP/2.0, HTTP/3.0]";
    assert!(
        message.contains(expected_prefix),
        "expected error message to contain {expected_prefix:?}, got: {message}"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn http2_only_with_single_supported_version_uses_prior_knowledge() {
    // A single supported version of HTTP/2 triggers
    // `hyper_builder.http2_only(true)` (HTTP/2 prior-knowledge mode).
    // Wiremock supports HTTP/2, so prior-knowledge succeeds and the response
    // arrives over HTTP/2. If prior-knowledge were not enabled, the client
    // would speak HTTP/1.1 and the post-connect protocol verification step
    // would reject the response, so this test pins down the prior-knowledge
    // behavior selected by the `len == 1 && [0] == HTTP_2` branch.
    let handler = HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(None), test_clock(), options(&[Version::HTTP_2]))
        .body_builder(HttpBodyBuilder::new_fake())
        .build(build_tls());

    let server = serve(Bytes::from_static(b"Hello World!")).await;

    let body_builder = HttpBodyBuilder::new_fake();
    let request = HttpRequestBuilder::new(&body_builder)
        .method(Method::GET)
        .uri(server.uri() + "/hello-world")
        .build()
        .unwrap();

    let response = handler.execute(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn single_http1_version_does_not_enable_http2_only() {
    // Builder sees a single supported version of HTTP/1.1. Since the version
    // is not HTTP/2, prior-knowledge mode must NOT be enabled, otherwise the
    // request would fail against an HTTP/1.1 server.
    let handler = HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(None), test_clock(), options(&[Version::HTTP_11]))
        .body_builder(HttpBodyBuilder::new_fake())
        .build(build_tls());

    let server = serve(Bytes::from_static(b"Hello World!")).await;

    let body_builder = HttpBodyBuilder::new_fake();
    let request = HttpRequestBuilder::new(&body_builder)
        .method(Method::GET)
        .uri(server.uri() + "/hello-world")
        .build()
        .unwrap();

    let response = handler.execute(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Crucially, the response is HTTP/1.1: prior-knowledge HTTP/2 must NOT
    // have been enabled (which would have made wiremock answer over HTTP/2).
    assert_eq!(response.version(), Version::HTTP_11);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn zero_lifetime_poisons_connection_after_request() {
    // ConnectionLifetime::Fixed(ZERO) makes every connection expired by the
    // time the response is delivered, exercising the poisoning branch in
    // hyper_handler::handle_poisoning that calls Connected::poison() and
    // ConnectionInfo::mark_poisoned(). This specific scenario requires
    // monotonically advancing real time between connection setup and
    // response delivery (`is_expired` is `age > max_age`, strictly), so a
    // controlled clock would have to be advanced from inside hyper-util's
    // pool — `Clock::new_tokio()` is used here as a deliberate exception.
    use fetch_options::ConnectionInfo;

    let mut opts = options(&[]);
    opts.connection_pool.connection_lifetime = fetch_options::ConnectionLifetime::fixed(Duration::ZERO);
    let handler = HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(None), Clock::new_tokio(), opts)
        .body_builder(HttpBodyBuilder::new_fake())
        .build(build_tls());

    let server = serve(Bytes::from_static(b"Hello World!")).await;

    let body_builder = HttpBodyBuilder::new_fake();
    let request = HttpRequestBuilder::new(&body_builder)
        .method(Method::GET)
        .uri(server.uri() + "/hello-world")
        .build()
        .unwrap();

    let response = handler.execute(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let info = response.extensions().get::<ConnectionInfo>().unwrap();
    assert!(info.is_poisoned(), "connection should have been poisoned by zero lifetime");
}

testing_aids::init_tracing!();
