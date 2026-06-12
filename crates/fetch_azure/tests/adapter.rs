// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for [`fetch_azure::FetchHttpClient`].
//!
//! These exercise the adapter end-to-end using `fetch`'s `FakeHandler`, so no
//! real network access is required.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use fetch::fake::FakeHandler;
use fetch::{HttpClient as FetchClient, HttpResponseBuilder};
use fetch_azure::{FetchHttpClient, new_http_client};
use futures::io::AsyncRead;
use typespec_client_core::Bytes;
use typespec_client_core::http::headers::HeaderName;
use typespec_client_core::http::request::{Body, Request};
use typespec_client_core::http::{HttpClient, Method, Url};
use typespec_client_core::stream::{BytesStream, SeekableStream};

fn request(method: Method) -> Request {
    Request::new(Url::parse("https://example.com/path").expect("valid url"), method)
}

/// A handler that always responds with the given status code and an empty body.
fn status_handler(status: u16) -> FakeHandler {
    FakeHandler::from_fn(move |_request| HttpResponseBuilder::new_fake().status(status).build())
}

#[tokio::test]
async fn execute_request_maps_status_headers_and_body() {
    let handler = FakeHandler::from_fn(|_request| {
        HttpResponseBuilder::new_fake()
            .status(201u16)
            .header("x-test", "hello")
            .text("world")
            .build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let response = client.execute_request(&request(Method::Get)).await.unwrap();

    assert_eq!(response.status(), 201u16);
    assert_eq!(response.headers().get_optional_str(&HeaderName::from("x-test")), Some("hello"));

    let body = response.into_body().collect().await.unwrap();
    assert_eq!(&*body, b"world");
}

#[tokio::test]
async fn execute_request_forwards_method_and_bytes_body() {
    // The handler echoes the request body back, but only for POST requests.
    let handler = FakeHandler::from_async_fn(|request| async move {
        if request.method().as_str() != "POST" {
            return HttpResponseBuilder::new_fake().status(400u16).build();
        }

        let body = request.into_body().into_bytes().await?;
        HttpResponseBuilder::new_fake().status(200u16).bytes(body).build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let mut request = request(Method::Post);
    request.set_body(Bytes::from_static(b"payload"));

    let response = client.execute_request(&request).await.unwrap();

    assert_eq!(response.status(), 200u16);
    let body = response.into_body().collect().await.unwrap();
    assert_eq!(&*body, b"payload");
}

#[tokio::test]
async fn execute_request_forwards_seekable_stream_body() {
    let handler = FakeHandler::from_async_fn(|request| async move {
        let body = request.into_body().into_bytes().await?;
        HttpResponseBuilder::new_fake().status(200u16).bytes(body).build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let mut request = request(Method::Put);
    request.set_body(BytesStream::new(Bytes::from_static(b"streamed")));

    let response = client.execute_request(&request).await.unwrap();

    assert_eq!(response.status(), 200u16);
    let body = response.into_body().collect().await.unwrap();
    assert_eq!(&*body, b"streamed");
}

#[tokio::test]
async fn execute_request_forwards_request_headers() {
    let handler = FakeHandler::from_fn(|request| {
        let forwarded = request.headers().get("x-correlation").and_then(|value| value.to_str().ok()) == Some("abc123");
        let status = if forwarded { 200u16 } else { 400u16 };
        HttpResponseBuilder::new_fake().status(status).build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let mut request = request(Method::Get);
    request.insert_header("x-correlation", "abc123");

    let response = client.execute_request(&request).await.unwrap();

    assert_eq!(response.status(), 200u16);
}

#[tokio::test]
async fn execute_request_maps_all_methods() {
    for method in [Method::Delete, Method::Get, Method::Head, Method::Patch, Method::Post, Method::Put] {
        let expected = method.as_str();
        let handler = FakeHandler::from_fn(move |request| {
            let status = if request.method().as_str() == expected { 200u16 } else { 400u16 };
            HttpResponseBuilder::new_fake().status(status).build()
        });
        let client = FetchHttpClient::new(FetchClient::new_fake(handler));

        let response = client.execute_request(&request(method)).await.unwrap();

        assert_eq!(response.status(), 200u16, "method {method:?} was not forwarded");
    }
}

#[tokio::test]
async fn execute_request_maps_transport_error() {
    let handler = FakeHandler::from_error_fn(|_request| fetch::HttpError::unavailable("simulated transport failure"));
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let error = client.execute_request(&request(Method::Get)).await.unwrap_err();

    assert!(
        error.to_string().contains("the fetch HTTP client failed to execute the request"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn new_http_client_returns_dyn_client() {
    let client = new_http_client(FetchClient::new_fake(status_handler(202)));

    let response = client.execute_request(&request(Method::Get)).await.unwrap();

    assert_eq!(response.status(), 202u16);
}

#[tokio::test]
async fn from_fetch_client_and_inner_round_trip() {
    let adapter = FetchHttpClient::from(FetchClient::new_fake(status_handler(200)));

    // `inner` exposes the wrapped client and `into_inner` returns it unchanged.
    let _ = adapter.inner();
    let recovered = adapter.into_inner();
    let adapter = FetchHttpClient::new(recovered);

    let response = adapter.execute_request(&request(Method::Get)).await.unwrap();
    assert_eq!(response.status(), 200u16);
}

#[tokio::test]
async fn execute_request_maps_request_build_failure() {
    let client = FetchHttpClient::new(FetchClient::new_fake(status_handler(200)));

    // A header value containing a control character is rejected by the `http`
    // crate when the fetch request is built, exercising the DataConversion path.
    let mut request = request(Method::Get);
    request.insert_header("x-invalid", "bad\nvalue");

    let error = client.execute_request(&request).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed to convert the Azure request into a fetch request"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn execute_request_skips_non_utf8_response_headers() {
    let handler = FakeHandler::from_fn(|_request| {
        let binary = fetch::HeaderValue::from_bytes(&[0xff, 0xfe]).expect("valid header value bytes");
        HttpResponseBuilder::new_fake()
            .status(200u16)
            .header("x-valid", "ok")
            .header("x-binary", binary)
            .build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let response = client.execute_request(&request(Method::Get)).await.unwrap();

    assert_eq!(response.headers().get_optional_str(&HeaderName::from("x-valid")), Some("ok"));
    assert_eq!(response.headers().get_optional_str(&HeaderName::from("x-binary")), None);
}

#[tokio::test]
async fn execute_request_maps_seekable_stream_read_error() {
    let handler = FakeHandler::from_async_fn(|request| async move {
        // Reading the body drives the erroring stream, surfacing the failure.
        request.into_body().into_bytes().await?;
        HttpResponseBuilder::new_fake().status(200u16).build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let mut request = request(Method::Post);
    request.set_body(Body::SeekableStream(Box::new(ErroringStream)));

    let error = client.execute_request(&request).await.unwrap_err();

    assert!(
        error_chain(&error).contains("failed to read the Azure request body"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn execute_request_maps_response_body_read_error() {
    let handler = FakeHandler::from_fn(|_request| {
        let body = fetch::HttpBodyBuilder::new_fake().stream(
            futures::stream::iter([Err(fetch::HttpError::unavailable("boom"))]),
            &fetch::options::HttpBodyOptions::default(),
        );
        HttpResponseBuilder::new_fake().status(200u16).body(body).build()
    });
    let client = FetchHttpClient::new(FetchClient::new_fake(handler));

    let response = client.execute_request(&request(Method::Get)).await.unwrap();
    let error = response.into_body().collect().await.unwrap_err();

    assert!(
        error.to_string().contains("failed to read the response body"),
        "unexpected error: {error}"
    );
}

/// A [`SeekableStream`] whose reads always fail, used to cover the request-body error path.
#[derive(Debug, Clone)]
struct ErroringStream;

impl AsyncRead for ErroringStream {
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::Error::other("boom")))
    }
}

#[async_trait]
impl SeekableStream for ErroringStream {
    async fn reset(&mut self) -> typespec_client_core::Result<()> {
        Ok(())
    }

    fn len(&self) -> Option<u64> {
        None
    }
}

/// Joins an error and its `source` chain into a single string for assertions.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain.push_str(" | ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}
