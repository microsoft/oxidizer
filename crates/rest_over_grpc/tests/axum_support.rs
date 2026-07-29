// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the `axum` integration (`IntoResponse`).
//!
//! Gated on `tower` too so `http-body-util` is available to collect the response
//! body for assertions.

#![cfg(all(feature = "axum", feature = "tower"))]
#![cfg(not(miri))] // the tokio runtime is unsupported under Miri.

use axum_core::response::IntoResponse as _;
use http_body_util::BodyExt as _;
use rest_over_grpc::handling::Status;
use rest_over_grpc::transcoding::HttpResponse;

#[tokio::test]
async fn http_response_into_axum_preserves_status_content_type_and_body() {
    let response = HttpResponse::from_status(&Status::not_found("gone")).into_response();

    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    assert_eq!(response.headers()[http::header::CONTENT_TYPE], "application/json");
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(value["message"], "gone");
}

#[tokio::test]
async fn streaming_response_into_axum_streams_frames() {
    use futures_util::stream;
    use rest_over_grpc::transcoding::{StreamEncoding, StreamingResponse};
    use serde::Serialize;

    #[derive(Serialize)]
    struct Msg {
        n: u32,
    }

    let items = stream::iter(vec![Ok::<_, Status>(Msg { n: 1 }), Ok(Msg { n: 2 })]);
    let response = StreamingResponse::encode(items, StreamEncoding::NdJson).into_response();

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.headers()[http::header::CONTENT_TYPE], "application/x-ndjson");
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"{\"n\":1}\n{\"n\":2}\n");
}

#[tokio::test]
async fn streaming_response_into_axum_applies_custom_headers() {
    use futures_util::stream;
    use rest_over_grpc::transcoding::{StreamEncoding, StreamingResponse};

    let mut streaming = StreamingResponse::encode(stream::iter(vec![Ok::<_, Status>(1_u32)]), StreamEncoding::JsonArray);
    let mut headers = http::HeaderMap::new();
    _ = headers.insert("x-trace", "abc".parse().expect("valid header value"));
    streaming.merge_headers(headers);

    let response = streaming.into_response();
    assert_eq!(response.headers()["x-trace"], "abc");
    assert_eq!(response.headers()[http::header::CONTENT_TYPE], "application/json");
}

#[tokio::test]
async fn transcode_response_into_axum_handles_both_variants() {
    use futures_util::stream;
    use rest_over_grpc::transcoding::{StreamEncoding, StreamingResponse, TranscodeResponse};

    let unary = TranscodeResponse::Unary(HttpResponse::ok_json(b"{}".to_vec())).into_response();
    assert_eq!(unary.status(), http::StatusCode::OK);

    let items = stream::iter(vec![Ok::<_, Status>(1_u32)]);
    let streaming = TranscodeResponse::Streaming(StreamingResponse::encode(items, StreamEncoding::NdJson)).into_response();
    assert_eq!(streaming.status(), http::StatusCode::OK);
    assert_eq!(streaming.headers()[http::header::CONTENT_TYPE], "application/x-ndjson");
    let body = streaming.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"1\n");
}

/// Compile-time guarantee that the adapters' response body satisfies `axum`'s
/// `Router::fallback_service` bound (`Service::Response: IntoResponse`).
///
/// This is what lets [`RestService`](rest_over_grpc::serving::RestService) mount
/// in an `axum::Router` with no handler glue, and what
/// [`serve_http`](rest_over_grpc::serving::serve_http)'s result can be returned
/// directly from a handler. It is the reason the adapters use
/// [`RestBody`](rest_over_grpc::serving::RestBody) (an [`http_body::Body`]) rather
/// than a bare `Vec<u8>`, which is *not* a `Body` and would fail this bound — so a
/// regression to such a type breaks the build here.
#[test]
fn adapter_response_body_is_axum_into_response() {
    fn assert_into_response<T: axum_core::response::IntoResponse>() {}

    assert_into_response::<http::Response<rest_over_grpc::serving::RestBody>>();
}
