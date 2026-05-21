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
use tick::Clock;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone)]
struct TokioConnector;

impl layered::Service<BaseUri> for TokioConnector {
    type Out = Result<TokioIo<tokio::net::TcpStream>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        let stream = tokio::net::TcpStream::connect((
            input.authority().host(),
            input.effective_port().expect("test BaseUri always has a port"),
        ))
        .await?;
        Ok(TokioIo::new(stream))
    }
}

fn build_tls() -> TlsBackend {
    native_tls::TlsConnector::new()
        .expect("default native-tls connector should build in tests")
        .into()
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

#[tokio::test]
async fn real_http_request_succeeds() {
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        Clock::new_tokio(),
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

#[tokio::test]
async fn https_only_filter_rejects_http_request() {
    // No `.request_filter(...)` call: defaults to `RequestFilter::Https`.
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        Clock::new_tokio(),
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

    let message = error.message();
    let expected_substring = "https required but URI was not https";
    assert!(
        message.contains(expected_substring),
        "expected error message to contain {expected_substring:?}, got: {message}"
    );
}

#[tokio::test]
async fn http2_only_rejected_when_server_negotiates_http1() {
    // Wiremock speaks plain HTTP/1.1. The client advertises only HTTP/2 and
    // HTTP/3 (so HTTP/2 prior-knowledge is not auto-enabled), causing
    // post-connect protocol verification to reject the HTTP/1.1 connection.
    let handler = HyperTransportBuilder::new(
        TokioConnector,
        Spawner::new_tokio(),
        Clock::new_tokio(),
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

    let message = error.message();
    let expected_substring =
        "the connection was established with unsupported HTTP version: HTTP/1.1, supported versions are: [HTTP/2.0, HTTP/3.0]";
    assert!(
        message.contains(expected_substring),
        "expected error message to contain {expected_substring:?}, got: {message}"
    );
}
