// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Smoke test verifying the builder produces a usable handler that can issue
//! a real HTTP request against a local mock server using the `native-tls`
//! backend and the `HttpAndHttps` request filter.

use std::time::Duration;

use anyspawn::Spawner;
use bytes::Bytes;
use fetch_hyper::{HyperTransportBuilder, RequestFilter, TlsBackend};
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
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        test_clock(),
        build_tls(),
        HttpBodyBuilder::new_fake(),
    )
    .connect_timeout(Duration::from_secs(5))
    .request_filter(RequestFilter::HttpAndHttps)
    .build();

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

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn https_only_filter_rejects_http_request() {
    // No `.request_filter(...)` call: defaults to `RequestFilter::Https`.
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        test_clock(),
        build_tls(),
        HttpBodyBuilder::new_fake(),
    )
    .connect_timeout(Duration::from_secs(5))
    .build();

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
        Spawner::new_tokio(),
        test_clock(),
        build_tls(),
        HttpBodyBuilder::new_fake(),
    )
    .connect_timeout(Duration::from_secs(5))
    .request_filter(RequestFilter::HttpAndHttps)
    .supported_http_versions(&[Version::HTTP_2, Version::HTTP_3])
    .build();

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
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        test_clock(),
        build_tls(),
        HttpBodyBuilder::new_fake(),
    )
    .connect_timeout(Duration::from_secs(5))
    .request_filter(RequestFilter::HttpAndHttps)
    .supported_http_versions(&[Version::HTTP_2])
    .build();

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
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        test_clock(),
        build_tls(),
        HttpBodyBuilder::new_fake(),
    )
    .connect_timeout(Duration::from_secs(5))
    .request_filter(RequestFilter::HttpAndHttps)
    .supported_http_versions(&[Version::HTTP_11])
    .build();

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
    use fetch_hyper::ConnectionInfo;

    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        Clock::new_tokio(),
        build_tls(),
        HttpBodyBuilder::new_fake(),
    )
    .connect_timeout(Duration::from_secs(5))
    .request_filter(RequestFilter::HttpAndHttps)
    .connection_lifetime(fetch_hyper::ConnectionLifetime::Fixed(Duration::ZERO))
    .build();

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
    assert!(info.poisoned(), "connection should have been poisoned by zero lifetime");
}
