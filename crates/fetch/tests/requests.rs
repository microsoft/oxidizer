// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests exercising real network requests through the HTTP client.

use std::time::Duration;

use bytes::Bytes;
use fetch::options::{ConnectionLifetime, ConnectionPoolOptions, PoolIndex};
use fetch::telemetry::ConnectionInfo;
use fetch::tokio::TokioDeps;
use fetch::{HttpClient, HttpClientBuilder};
use http::{StatusCode, Version};
use http_extensions::HttpBodyOptions;
use ohno::{ErrorExt, assert_error_message};
use tick::ClockControl;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn response_ok() {
    let client = create_builder().build();

    let server = serve(Bytes::from("Hello World!")).await;
    let response = client.get(server.uri() + "/hello-world").fetch_text().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn response_buffering_limit() {
    let client = create_builder()
        .response_body_options(HttpBodyOptions::default().buffer_limit(1024))
        .build();
    let content = vec![0; 1025];
    let server = serve(Bytes::from(content)).await;

    let error = client.get(server.uri() + "/hello-world").fetch_buffered().await.unwrap_err();

    assert_eq!(error.message(), "body size exceeds the limit of 1024 bytes");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn no_tls_http_1_selected() {
    let client = create_builder().build();
    let server = serve(Bytes::from_static(b"hello")).await;

    let version = client.get(server.uri() + "/hello-world").fetch().await.unwrap().version();

    assert_eq!(version, Version::HTTP_11);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn no_tls_http_2_accepted() {
    let client = create_builder().supported_http_versions(&[Version::HTTP_2]).build();
    let server = serve(Bytes::from_static(b"hello")).await;

    let _response = client.get(server.uri() + "/hello-world").fetch().await.unwrap();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn no_tls_http_2_rejected() {
    let client = create_builder()
        .supported_http_versions(&[Version::HTTP_2, Version::HTTP_3])
        .build();
    let server = serve(Bytes::from_static(b"hello")).await;

    let error = client.get(server.uri() + "/hello-world").fetch().await.unwrap_err();

    assert_error_message!(error, "client error (Connect)");
    error.find_source_with::<fetch::HttpError>(|e| {
        e.message()
            == "the connection was established with unsupported HTTP version: HTTP/1.1, supported versions are: [HTTP/2.0, HTTP/3.0]"
    });
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn no_tls_http_3_rejected() {
    let client = create_builder()
        .supported_http_versions(&[Version::HTTP_2, Version::HTTP_3])
        .build();
    let server = serve(Bytes::from_static(b"hello")).await;

    let error = client.get(server.uri() + "/hello-world").fetch().await.unwrap_err();

    assert_error_message!(error, "client error (Connect)");
    error.find_source_with::<fetch::HttpError>(|e| {
        e.message()
            == "the connection was established with unsupported HTTP version: HTTP/1.1, supported versions are: [HTTP/2.0, HTTP/3.0]"
    });
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
#[cfg(feature = "json")]
async fn json_owned() {
    use serde::Deserialize;
    #[derive(Deserialize, Debug)]
    struct Person {
        name: String,
        surname: String,
    }

    let json = Bytes::from_static(br#"{"name": "John", "surname": "Doe"}"#);
    let client = create_builder().build();
    let server = serve(json).await;

    let person = client
        .get(server.uri() + "/hello-world")
        .fetch_json_owned::<Person>()
        .await
        .unwrap()
        .into_body();

    assert_eq!(person.name, "John");
    assert_eq!(person.surname, "Doe");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
#[cfg(feature = "json")]
async fn json_borrowed() {
    use serde::Deserialize;
    #[derive(Deserialize, Debug)]
    struct Person<'a> {
        name: std::borrow::Cow<'a, str>,
        surname: std::borrow::Cow<'a, str>,
    }

    let json = Bytes::from_static(br#"{"name": "John", "surname": "Doe"}"#);
    let client = create_builder().build();
    let server = serve(json).await;

    let mut json = client
        .get(server.uri() + "/hello-world")
        .fetch_json::<Person>()
        .await
        .unwrap()
        .into_body();

    let person = json.read().unwrap();
    assert_eq!(person.name, "John");
    assert_eq!(person.surname, "Doe");
}

/// Verifies two related guarantees of the real network pipeline:
///
/// 1. Every response served by a real connection carries a [`ConnectionInfo`]
///    extension whose configured `pool_index` and `max_age` match the client.
/// 2. Once the connection's age exceeds the configured maximum lifetime, the
///    handler poisons it so the pool drops it; subsequent requests are then
///    served by a freshly established connection.
///
/// Time is driven by a [`ClockControl`] so the test does not need to sleep for
/// real wall-clock durations.
#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn connection_info_attached_and_recreated_after_max_lifetime() {
    let control = ClockControl::new();
    let max_age = Duration::from_mins(1);
    let client = HttpClient::builder_tokio(TokioDeps::with_clock(&control.to_clock()))
        .insecure_allow_http()
        .connection_pool_options(ConnectionPoolOptions::default().connection_lifetime(ConnectionLifetime::fixed(max_age)))
        .build();

    let server = serve(Bytes::from_static(b"hello")).await;
    let url = server.uri() + "/hello-world";

    // Fresh connection: `ConnectionInfo` must be attached, carry the configured
    // pool index, and report a non-poisoned, brand-new connection.
    let response = client.get(url.clone()).fetch().await.unwrap();
    let info = response
        .extensions()
        .get::<ConnectionInfo>()
        .expect("ConnectionInfo must be attached to every response served by a real connection");
    assert_eq!(info.pool_index(), PoolIndex::new(0));
    assert!(!info.is_poisoned());

    // Drive the controlled clock past the configured max lifetime. The next
    // request reuses the pooled connection one final time; the handler observes
    // the age overflow and poisons the connection so the pool drops it.
    control.advance(max_age + Duration::from_secs(1));

    let response = client.get(url.clone()).fetch().await.unwrap();
    let expired_info = response
        .extensions()
        .get::<ConnectionInfo>()
        .expect("ConnectionInfo must be attached to every response served by a real connection");
    assert!(
        expired_info.is_poisoned(),
        "expected the connection to be poisoned once its age exceeded max_age",
    );

    // Subsequent requests must be served by a freshly established connection
    // whose `ConnectionInfo` is not poisoned.
    let response = client.get(url).fetch().await.unwrap();
    let fresh_info = response
        .extensions()
        .get::<ConnectionInfo>()
        .expect("ConnectionInfo must be attached to every response served by a real connection");
    assert!(!fresh_info.is_poisoned(), "a newly established connection must not be poisoned");
}

fn create_builder() -> HttpClientBuilder {
    HttpClient::builder_tokio(TokioDeps::default()).insecure_allow_http()
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
