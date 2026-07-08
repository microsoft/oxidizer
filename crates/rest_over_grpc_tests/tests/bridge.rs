// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Proves the `rest_over_grpc::build` `tonic` bridge works end to end: a service
//! implemented only against tonic's generated `greeter_server::Greeter` trait is,
//! via the generated blanket `impl`, also a `rest_over_grpc` service whose
//! `transcode` transcodes REST/JSON requests.

use futures::StreamExt as _;
use futures::executor::block_on;
use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
use rest_over_grpc_tests::tonic_bridge::{GreeterService, Transcoder};

/// Unwraps a unary [`TranscodeResponse`] into its buffered [`HttpResponse`].
fn unary(response: TranscodeResponse) -> HttpResponse {
    match response {
        TranscodeResponse::Unary(http) => Some(http),
        TranscodeResponse::Streaming(_) => None,
    }
    .expect("expected a unary response")
}

/// Collects a streaming [`TranscodeResponse`]'s frames, returning its content
/// type and concatenated body bytes.
fn streaming(response: TranscodeResponse) -> (String, Vec<u8>) {
    let stream = match response {
        TranscodeResponse::Streaming(stream) => Some(stream),
        TranscodeResponse::Unary(_) => None,
    }
    .expect("expected a streaming response");
    let content_type = stream.content_type().to_owned();
    let body = block_on(async {
        let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
        frames.concat()
    });
    (content_type, body)
}

#[test]
fn tonic_bridge_transcodes_unary() {
    // `GreeterService` only implements the tonic trait; the generated bridge makes
    // it a REST service, so `transcode` transcodes the `GET /v1/greet/{name}` route.
    let response = unary(block_on(Transcoder::new(GreeterService).transcode(
        "GET",
        "/v1/greet/World",
        http::HeaderMap::new(),
        b"",
    )));
    assert_eq!(response.status(), http::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(response.body()).expect("valid JSON body");
    assert_eq!(body["message"], "Hello, World!");
}

#[test]
fn tonic_bridge_maps_handler_status_to_not_found() {
    // A handler-returned `tonic::Status::not_found` (for the `missing` sentinel)
    // is bridged to a REST 404 carrying the handler's message — distinct from an
    // unmatched route's generic 404.
    let response = unary(block_on(Transcoder::new(GreeterService).transcode(
        "GET",
        "/v1/greet/missing",
        http::HeaderMap::new(),
        b"",
    )));
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(response.body()).expect("valid JSON body");
    assert_eq!(body["message"], "no greeting for that name");
}

#[test]
fn tonic_bridge_maps_status_to_not_found() {
    // An unmatched route falls through to a 404 via the generated `transcode`.
    let response = unary(block_on(Transcoder::new(GreeterService).transcode(
        "GET",
        "/v1/nope",
        http::HeaderMap::new(),
        b"",
    )));
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
}

#[test]
fn tonic_bridge_transcodes_server_streaming_as_json_array() {
    // The server-streaming RPC is bridged too; the streaming `transcode` yields
    // frames that concatenate into a JSON array (the default encoding).
    let (content_type, body) = streaming(block_on(Transcoder::new(GreeterService).transcode(
        "GET",
        "/v1/greet/World:stream",
        http::HeaderMap::new(),
        b"",
    )));
    assert_eq!(content_type, "application/json");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON array body");
    assert_eq!(body[0]["message"], "Hello, World!");
    assert_eq!(body[1]["message"], "Bye, World!");
}

#[test]
fn tonic_bridge_negotiates_ndjson_for_streaming() {
    // An `Accept: application/x-ndjson` request gets newline-delimited JSON.
    let mut headers = http::HeaderMap::new();
    let _ = headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("application/x-ndjson"));
    let (content_type, body) = streaming(block_on(Transcoder::new(GreeterService).transcode(
        "GET",
        "/v1/greet/World:stream",
        headers,
        b"",
    )));
    assert_eq!(content_type, "application/x-ndjson");
    let text = String::from_utf8(body).expect("utf8 body");
    assert_eq!(text, "{\"message\":\"Hello, World!\"}\n{\"message\":\"Bye, World!\"}\n");
}
