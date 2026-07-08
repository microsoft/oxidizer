// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end transcoding tests: real `pbjson` messages flowing through the
//! generated REST router, service trait, and transcoder.

#![cfg(not(miri))] // the tokio runtime is unsupported under Miri.

use futures::StreamExt as _;
use http::{HeaderMap, HeaderValue};
use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
use rest_over_grpc_tests::custom::{InMemoryLibrary, Transcoder};
use serde_json::Value;

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
async fn streaming(response: TranscodeResponse) -> (String, Vec<u8>) {
    let stream = match response {
        TranscodeResponse::Streaming(stream) => Some(stream),
        TranscodeResponse::Unary(_) => None,
    }
    .expect("expected a streaming response");
    let content_type = stream.content_type().to_owned();
    let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
    (content_type, frames.concat())
}

fn body(value: &HttpResponse) -> Value {
    serde_json::from_slice(value.body()).expect("response body is valid JSON")
}

/// Builds a request header map carrying a single `Accept` value.
fn accept(value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    _ = headers.insert("accept", HeaderValue::from_static(value));
    headers
}

#[tokio::test]
async fn get_shelf_binds_path_variable() {
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/history", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    assert_eq!(json["name"], "shelves/history");
    assert_eq!(json["theme"], "history");
}

#[tokio::test]
async fn create_shelf_binds_body_field() {
    let payload = br#"{"name":"ignored","theme":"sci-fi"}"#;
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("POST", "/v1/shelves", HeaderMap::new(), payload)
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    assert_eq!(json["name"], "shelves/created");
    assert_eq!(json["theme"], "sci-fi");
}

#[tokio::test]
async fn list_shelves_binds_query_parameter() {
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves?filter=science", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    let shelves = json["shelves"].as_array().expect("array");
    assert_eq!(shelves.len(), 1);
    assert_eq!(shelves[0]["theme"], "science");
}

#[tokio::test]
async fn list_shelves_by_genre_binds_enum_path_variable_by_name() {
    // The `{genre}` path variable targets an `enum` field: given by name, it is
    // decoded to the matching enum value (proto3 JSON parity).
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/genre/SCIENCE", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    let shelves = json["shelves"].as_array().expect("array");
    assert_eq!(shelves.len(), 1);
    assert_eq!(shelves[0]["theme"], "science");
}

#[tokio::test]
async fn list_shelves_by_genre_binds_enum_path_variable_by_number() {
    // The same enum path variable also accepts the value's number.
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/genre/1", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    let shelves = json["shelves"].as_array().expect("array");
    assert_eq!(shelves.len(), 1);
    assert_eq!(shelves[0]["theme"], "history");
}

#[tokio::test]
async fn list_shelves_by_genre_rejects_an_unknown_enum_value() {
    // A value that is neither a known name nor a number is an invalid argument.
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/genre/BOGUS", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn stream_shelves_renders_json_array() {
    let (content_type, bytes) = streaming(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves:stream", HeaderMap::new(), b"")
            .await,
    )
    .await;
    assert_eq!(content_type, "application/json");
    let json: Value = serde_json::from_slice(&bytes).expect("streamed JSON array");
    let shelves = json.as_array().expect("streamed JSON array");
    assert_eq!(shelves.len(), 2);
    assert_eq!(shelves[0]["name"], "shelves/1");
}

#[tokio::test]
async fn stream_shelves_filters_via_query() {
    let (_content_type, bytes) = streaming(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves:stream?filter=science", HeaderMap::new(), b"")
            .await,
    )
    .await;
    let json: Value = serde_json::from_slice(&bytes).expect("streamed JSON array");
    let shelves = json.as_array().expect("streamed JSON array");
    assert_eq!(shelves.len(), 1);
    assert_eq!(shelves[0]["theme"], "science");
}

#[tokio::test]
async fn stream_shelves_negotiates_encoding_via_accept() {
    // NDJSON: one compact JSON object per line.
    let (ndjson_type, ndjson_bytes) = streaming(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves:stream", accept("application/x-ndjson"), b"")
            .await,
    )
    .await;
    assert_eq!(ndjson_type, "application/x-ndjson");
    let line_count = ndjson_bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()).count();
    assert_eq!(line_count, 2);

    // SSE: one `data:` frame per message.
    let (sse_type, sse_bytes) = streaming(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves:stream", accept("text/event-stream"), b"")
            .await,
    )
    .await;
    assert_eq!(sse_type, "text/event-stream");
    assert!(std::str::from_utf8(&sse_bytes).expect("utf8").starts_with("data: "));
}

#[tokio::test]
async fn typed_query_fields_flow_through_generated_pbjson_messages() {
    let target = concat!(
        "/v1/shelves:search?",
        "include_archived=true&tags=first&tags=second&genre=2&threshold=1.5&",
        "options%2Eenabled=true&numeric_text=2"
    );
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", target, HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200, "{}", String::from_utf8_lossy(resp.body()));
    let json = body(&resp);
    assert_eq!(json["includeArchived"], true);
    assert_eq!(json["tags"], serde_json::json!(["first", "second"]));
    assert_eq!(json["genre"], "SCIENCE");
    assert_eq!(json["threshold"], 1.5);
    assert_eq!(json["options"]["enabled"], true);
    assert_eq!(json["numericText"], "2");
}

#[tokio::test]
async fn malformed_query_encoding_and_noncanonical_float_are_rejected() {
    for target in [
        "/v1/shelves:search?numeric_text=%FF",
        "/v1/shelves:search?numeric%zztext=x",
        "/v1/shelves:search?threshold=inf",
        "/v1/shelves:search?include_archived=true&include_archived=false",
    ] {
        let resp = unary(
            Transcoder::new(InMemoryLibrary)
                .transcode("GET", target, HeaderMap::new(), b"")
                .await,
        );
        assert_eq!(resp.status().as_u16(), 400, "{target}");
    }
}

#[tokio::test]
async fn additional_binding_uses_its_own_body_and_response_body() {
    let primary = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("PATCH", "/v1/shelves/primary", HeaderMap::new(), br#"{"theme":"history"}"#)
            .await,
    );
    assert_eq!(body(&primary), serde_json::json!({"name":"shelves/primary","theme":"history"}));

    let additional = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode(
                "POST",
                "/v1/shelves/secondary:replace",
                HeaderMap::new(),
                br#"{"shelf":{"theme":"science"},"force":true}"#,
            )
            .await,
    );
    assert_eq!(body(&additional), serde_json::json!(1));
}

#[tokio::test]
async fn streaming_response_body_selects_each_item_field() {
    let (content_type, bytes) = streaming(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelfThemes:stream", HeaderMap::new(), b"")
            .await,
    )
    .await;
    assert_eq!(content_type, "application/json");
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).expect("streamed JSON"),
        serde_json::json!(["history", "science"])
    );
}

#[tokio::test]
async fn multi_segment_capture_preserves_encoded_slash_and_rejects_bad_encoding() {
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/tree/a%2Fb%20c", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(body(&resp)["path"], "a%2Fb c");

    for target in ["/v1/tree/%FF", "/v1/tree/%zz"] {
        let resp = unary(
            Transcoder::new(InMemoryLibrary)
                .transcode("GET", target, HeaderMap::new(), b"")
                .await,
        );
        assert_eq!(resp.status().as_u16(), 400, "{target}");
    }
}

#[tokio::test]
async fn streaming_accept_quality_excludes_zero_quality_encoding() {
    let (content_type, _) = streaming(
        Transcoder::new(InMemoryLibrary)
            .transcode(
                "GET",
                "/v1/shelves:stream",
                accept("text/event-stream;q=0, application/x-ndjson;q=0.5"),
                b"",
            )
            .await,
    )
    .await;
    assert_eq!(content_type, "application/x-ndjson");
}

#[tokio::test]
async fn unknown_route_yields_404() {
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/widgets/1", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn try_transcode_distinguishes_unmatched_routes() {
    // An unmatched route yields `None`, so a caller can supply its own fallback.
    assert!(
        Transcoder::new(InMemoryLibrary)
            .try_transcode("GET", "/v1/widgets/1", HeaderMap::new(), b"")
            .await
            .is_none()
    );
    // A matched route yields `Some`, even when the handler itself fails.
    let matched = Transcoder::new(InMemoryLibrary)
        .try_transcode("GET", "/v1/shelves/history", HeaderMap::new(), b"")
        .await;
    assert_eq!(unary(matched.expect("route matched")).status().as_u16(), 200);
    let missing = Transcoder::new(InMemoryLibrary)
        .try_transcode("GET", "/v1/shelves/missing", HeaderMap::new(), b"")
        .await;
    assert_eq!(unary(missing.expect("route matched")).status().as_u16(), 404);
}

#[tokio::test]
async fn handler_status_maps_to_http() {
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/missing", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 404);
    let json = body(&resp);
    assert_eq!(json["message"], "no such shelf");
}

#[tokio::test]
async fn method_disambiguates_same_path() {
    let get = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves", HeaderMap::new(), b"")
            .await,
    );
    assert!(body(&get)["shelves"].is_array());

    let payload = br#"{"theme":"poetry"}"#;
    let post = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("POST", "/v1/shelves", HeaderMap::new(), payload)
            .await,
    );
    assert_eq!(body(&post)["theme"], "poetry");
}

#[tokio::test]
async fn handler_sets_response_headers_and_echoes_request_metadata() {
    let mut request = HeaderMap::new();
    _ = request.insert("x-trace-id", HeaderValue::from_static("trace-42"));

    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/history", request, b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    // A cache validator set by the handler and the echoed request metadata both
    // surface as response headers.
    assert_eq!(resp.headers()["etag"], "\"shelf-v1\"");
    assert_eq!(resp.headers()["x-trace-id"], "trace-42");
    // The JSON content type stays authoritative alongside the custom headers.
    assert_eq!(resp.content_type(), "application/json");
}

#[tokio::test]
async fn create_sets_location_header() {
    let payload = br#"{"theme":"sci-fi"}"#;
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("POST", "/v1/shelves", HeaderMap::new(), payload)
            .await,
    );
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers()["location"], "/v1/shelves/created");
}

#[tokio::test]
async fn error_status_carries_details() {
    let resp = unary(
        Transcoder::new(InMemoryLibrary)
            .transcode("GET", "/v1/shelves/missing", HeaderMap::new(), b"")
            .await,
    );
    assert_eq!(resp.status().as_u16(), 404);
    let json = body(&resp);
    assert_eq!(json["message"], "no such shelf");
    let details = json["details"].as_array().expect("details array");
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["resourceName"], "shelves/missing");
}
