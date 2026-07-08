// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for server-streaming responses through the HTTP
//! adapter (`serve_http_fn`, `serve_http`, and `RestService`), which forward
//! streaming frames to the wire incrementally while still buffering unary
//! responses.

#![cfg(feature = "tower")]
#![cfg(not(miri))] // the tokio runtime is unsupported under Miri.

use futures_util::stream;
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::codegen_helpers::StreamEncoding;
use rest_over_grpc::handling::Status;
use rest_over_grpc::serving::{RestService, serve_http_fn};
use rest_over_grpc::transcoding::{HttpResponse, StreamingResponse, TranscodeResponse};
use serde::Serialize;
use tower_service::Service as _;

#[derive(Serialize)]
struct Tick {
    n: u32,
}

fn request(accept: &str) -> http::Request<Full<bytes::Bytes>> {
    http::Request::builder()
        .method("GET")
        .uri("/x")
        .header(http::header::ACCEPT, accept)
        .body(Full::new(bytes::Bytes::new()))
        .expect("valid request")
}

/// A hand-written [`Transcode`](rest_over_grpc::transcoding::Transcode) that
/// always answers with a server-streaming response, used to exercise the
/// streaming path of the service.
#[derive(Clone)]
struct TickStream;

impl rest_over_grpc::transcoding::Transcode for TickStream {
    fn try_transcode(
        &self,
        _method: &str,
        _target: &str,
        _headers: http::HeaderMap,
        _body: &[u8],
    ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
        let items = stream::iter(vec![Ok::<_, Status>(Tick { n: 9 })]);
        async move {
            Some(TranscodeResponse::Streaming(StreamingResponse::encode(
                items,
                StreamEncoding::JsonArray,
            )))
        }
    }
}

#[tokio::test]
async fn serve_http_fn_streams_ndjson_frames() {
    let response = serve_http_fn(request("application/x-ndjson"), |_method, _uri, _headers, _body| async {
        let items = stream::iter(vec![Ok::<_, Status>(Tick { n: 1 }), Ok(Tick { n: 2 })]);
        TranscodeResponse::Streaming(StreamingResponse::encode(items, StreamEncoding::NdJson))
    })
    .await;

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.headers()[http::header::CONTENT_TYPE], "application/x-ndjson");
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"{\"n\":1}\n{\"n\":2}\n");
}

#[tokio::test]
async fn serve_http_fn_applies_response_headers_and_content_type() {
    let response = serve_http_fn(request("application/json"), |_method, _uri, _headers, _body| async {
        let mut streaming = StreamingResponse::encode(stream::iter(vec![Ok::<_, Status>(Tick { n: 7 })]), StreamEncoding::JsonArray);
        let mut headers = http::HeaderMap::new();
        _ = headers.insert("x-trace", "abc".parse().expect("valid header value"));
        streaming.merge_headers(headers);
        TranscodeResponse::Streaming(streaming)
    })
    .await;

    assert_eq!(response.headers()["x-trace"], "abc");
    // `Content-Type` stays authoritative from the encoding.
    assert_eq!(response.headers()[http::header::CONTENT_TYPE], "application/json");
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"[{\"n\":7}]");
}

#[tokio::test]
async fn serve_http_fn_buffers_unary_responses() {
    let response = serve_http_fn(request("*/*"), |_method, _uri, _headers, _body| async {
        TranscodeResponse::Unary(HttpResponse::ok_json(br#"{"ok":true}"#.to_vec()))
    })
    .await;

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.headers()[http::header::CONTENT_TYPE], "application/json");
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), br#"{"ok":true}"#);
}

#[tokio::test]
async fn serve_http_fn_terminates_body_on_mid_stream_error() {
    let response = serve_http_fn(request("application/x-ndjson"), |_method, _uri, _headers, _body| async {
        let items = stream::iter(vec![Ok::<_, Status>(Tick { n: 1 }), Err(Status::internal("boom"))]);
        TranscodeResponse::Streaming(StreamingResponse::encode(items, StreamEncoding::NdJson))
    })
    .await;

    // The status line is already `200` before the error is observed, so the
    // failure can only surface as a truncated (error-terminated) body.
    assert_eq!(response.status(), http::StatusCode::OK);
    let collected = response.into_body().collect().await;
    assert!(collected.is_err(), "a mid-stream error terminates the body");
}

#[tokio::test]
async fn rest_service_serves_frames() {
    let mut service = RestService::new(TickStream);

    let response = service.call(request("application/json")).await.expect("infallible");
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"[{\"n\":9}]");
}

#[cfg(feature = "layered")]
#[tokio::test]
async fn rest_service_serves_frames_via_layered() {
    use layered::Service as _;

    let service = RestService::new(TickStream);

    let response = service.execute(request("application/json")).await;
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"[{\"n\":9}]");
}

#[tokio::test]
async fn serve_http_wires_a_streaming_transcode_impl() {
    use rest_over_grpc::serving::serve_http;

    // The bare `serve_http` free function (not the `_fn` closure form and not the
    // service wrapper) delegates to the uncapped read path.
    let response = serve_http(request("application/json"), &TickStream).await;
    let body = response.into_body().collect().await.expect("body collects").to_bytes();
    assert_eq!(body.as_ref(), b"[{\"n\":9}]");
}
