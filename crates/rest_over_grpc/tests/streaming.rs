// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the server-streaming response encoders.

#![cfg(not(miri))] // the tokio runtime is unsupported under Miri.

use futures_util::{Stream, StreamExt as _, stream};
use rest_over_grpc::codegen_helpers::{StreamEncoding, encode_frames, map_stream_status};
use rest_over_grpc::handling::{Code, ResponseStream, Status};
use rest_over_grpc::transcoding::{HttpResponse, StreamingResponse, TranscodeResponse};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct Msg {
    n: u32,
}

/// A type whose `Serialize` impl always fails, exercising the mid-stream
/// serialization-error path.
struct BadMsg;

impl Serialize for BadMsg {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("intentional serialize failure"))
    }
}

fn ok_stream(ns: &[u32]) -> impl Stream<Item = Result<Msg, Status>> {
    let items: Vec<Result<Msg, Status>> = ns.iter().map(|&n| Ok(Msg { n })).collect();
    stream::iter(items)
}

async fn collect(ns: &[u32], encoding: StreamEncoding) -> String {
    let frames: Vec<Vec<u8>> = encode_frames(ok_stream(ns), encoding).map(|f| f.expect("frame")).collect().await;
    String::from_utf8(frames.concat()).expect("utf8")
}

#[tokio::test]
async fn json_array_encodes_all_messages() {
    assert_eq!(collect(&[1, 2, 3], StreamEncoding::JsonArray).await, r#"[{"n":1},{"n":2},{"n":3}]"#);
}

#[tokio::test]
async fn json_array_empty_stream_is_empty_array() {
    assert_eq!(collect(&[], StreamEncoding::JsonArray).await, "[]");
}

#[tokio::test]
async fn ndjson_encodes_one_object_per_line() {
    assert_eq!(collect(&[1, 2], StreamEncoding::NdJson).await, "{\"n\":1}\n{\"n\":2}\n");
}

#[tokio::test]
async fn ndjson_empty_stream_is_empty() {
    assert_eq!(collect(&[], StreamEncoding::NdJson).await, "");
}

#[tokio::test]
async fn sse_wraps_each_message_in_a_data_frame() {
    assert_eq!(collect(&[7], StreamEncoding::Sse).await, "data: {\"n\":7}\n\n");
}

#[tokio::test]
async fn frames_concatenate_to_the_collected_body() {
    let frames: Vec<Vec<u8>> = encode_frames(ok_stream(&[1, 2]), StreamEncoding::JsonArray)
        .map(|f| f.expect("frame"))
        .collect()
        .await;
    let joined: Vec<u8> = frames.concat();
    assert_eq!(String::from_utf8(joined).expect("utf8"), r#"[{"n":1},{"n":2}]"#);
}

#[tokio::test]
async fn error_in_stream_is_propagated() {
    let items = stream::iter(vec![Ok(Msg { n: 1 }), Err(Status::internal("boom"))]);
    let results: Vec<Result<Vec<u8>, Status>> = encode_frames(items, StreamEncoding::JsonArray).collect().await;
    let err = results.into_iter().find_map(Result::err).expect("propagates");
    assert_eq!(err.message(), "boom");
}

#[test]
fn content_types_match_encodings() {
    assert_eq!(StreamEncoding::default(), StreamEncoding::JsonArray);
    assert_eq!(StreamEncoding::JsonArray.content_type(), "application/json");
    assert_eq!(StreamEncoding::NdJson.content_type(), "application/x-ndjson");
    assert_eq!(StreamEncoding::Sse.content_type(), "text/event-stream");
}

#[test]
fn from_accept_negotiates_the_encoding() {
    assert_eq!(StreamEncoding::from_accept("text/event-stream"), StreamEncoding::Sse);
    assert_eq!(StreamEncoding::from_accept("application/x-ndjson"), StreamEncoding::NdJson);
    assert_eq!(StreamEncoding::from_accept("application/jsonl"), StreamEncoding::NdJson);
    assert_eq!(StreamEncoding::from_accept("application/json"), StreamEncoding::JsonArray);
    assert_eq!(StreamEncoding::from_accept("*/*"), StreamEncoding::JsonArray);
    assert_eq!(StreamEncoding::from_accept(""), StreamEncoding::JsonArray);
    // Quality values take precedence over header order.
    assert_eq!(
        StreamEncoding::from_accept("application/json, text/event-stream;q=0.9"),
        StreamEncoding::JsonArray
    );
    assert_eq!(
        StreamEncoding::from_accept("text/event-stream ; charset=utf-8"),
        StreamEncoding::Sse
    );
}

#[tokio::test]
async fn serialization_failure_is_reported_as_internal() {
    let items = stream::iter(vec![Ok(BadMsg)]);
    let results: Vec<Result<Vec<u8>, Status>> = encode_frames(items, StreamEncoding::JsonArray).collect().await;
    let err = results.into_iter().find_map(Result::err).expect("serialize fails");
    assert_eq!(err.code(), Code::Internal);
}

// Mirrors the server-streaming bridge contract: a two-phase
// `async fn -> Result<ResponseStream, Status>` that maps the initiation `Status`
// and error-maps the response stream via `map_stream_status`.

struct FakeStatus(String);

fn convert_fake(status: FakeStatus) -> Status {
    Status::new(Code::Internal, status.0)
}

struct FakeResponse<T>(T);

impl<T> FakeResponse<T> {
    fn into_inner(self) -> T {
        self.0
    }
}

struct FakeService {
    ok: bool,
}

impl FakeService {
    async fn stream_things(
        &self,
        start: u32,
    ) -> Result<FakeResponse<impl Stream<Item = Result<u32, FakeStatus>> + Send + 'static>, FakeStatus> {
        // Mirror `tonic`'s genuinely-async initiation.
        core::future::ready(()).await;
        if self.ok {
            Ok(FakeResponse(stream::iter(vec![Ok(start), Err(FakeStatus("mid".to_owned()))])))
        } else {
            Err(FakeStatus("init".to_owned()))
        }
    }
}

async fn bridge_call(service: &FakeService, start: u32) -> Result<ResponseStream<u32>, Status> {
    let stream = service
        .stream_things(start)
        .await
        .map(FakeResponse::into_inner)
        .map_err(convert_fake)?;
    Ok(Box::pin(map_stream_status(stream, convert_fake)))
}

#[tokio::test]
async fn bridge_streams_success_and_converts_item_errors() {
    let service = FakeService { ok: true };
    let stream = bridge_call(&service, 5).await.expect("initiation succeeds");
    let out: Vec<Result<u32, Status>> = stream.collect().await;
    assert_eq!(out.len(), 2);
    assert_eq!(*out[0].as_ref().expect("first item"), 5);
    let err = out[1].as_ref().expect_err("second item is an error");
    assert_eq!(err.code(), Code::Internal);
    assert_eq!(err.message(), "mid");
}

#[tokio::test]
async fn bridge_surfaces_initiation_error_as_status() {
    let service = FakeService { ok: false };
    let Err(err) = bridge_call(&service, 0).await else {
        panic!("initiation should fail");
    };
    assert_eq!(err.message(), "init");
}

#[test]
fn bridge_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let service = FakeService { ok: true };
    assert_send(&bridge_call(&service, 1));
}

// Real-streaming value types (`StreamingResponse` / `TranscodeResponse`).

#[tokio::test]
async fn streaming_response_encodes_ndjson_frames_lazily() {
    let items = stream::iter(vec![Ok::<_, Status>(Msg { n: 1 }), Ok(Msg { n: 2 })]);
    let response = StreamingResponse::encode(items, StreamEncoding::NdJson);
    assert_eq!(response.content_type(), "application/x-ndjson");
    let frames: Vec<Vec<u8>> = response.into_frames().map(|frame| frame.expect("frame")).collect().await;
    assert_eq!(String::from_utf8(frames.concat()).expect("utf8"), "{\"n\":1}\n{\"n\":2}\n");
}

#[tokio::test]
async fn streaming_response_applies_response_body_mapping() {
    let items = stream::iter(vec![Ok::<_, Status>(Msg { n: 7 })]);
    let response = StreamingResponse::encode_response(
        items,
        StreamEncoding::JsonArray,
        rest_over_grpc::codegen_helpers::ResponseBodyKind::Field("n"),
    );
    let frames: Vec<Vec<u8>> = response.into_frames().map(|frame| frame.expect("frame")).collect().await;
    assert_eq!(frames.concat(), b"[7]");
}

#[test]
fn streaming_response_merges_response_headers() {
    let mut response = StreamingResponse::new(
        http::HeaderValue::from_static("application/json"),
        stream::empty::<Result<Vec<u8>, Status>>(),
    );
    let mut headers = http::HeaderMap::new();
    _ = headers.insert("x-trace", "abc".parse().expect("valid header value"));
    response.merge_headers(headers);
    assert_eq!(response.headers().get("x-trace").expect("header present"), "abc");
}

#[test]
fn transcode_response_from_conversions_pick_the_variant() {
    let unary: TranscodeResponse = HttpResponse::ok_json(b"{}".to_vec()).into();
    assert!(matches!(unary, TranscodeResponse::Unary(_)));

    let empty = stream::iter(Vec::<Result<Msg, Status>>::new());
    let streaming: TranscodeResponse = StreamingResponse::encode(empty, StreamEncoding::JsonArray).into();
    assert!(matches!(streaming, TranscodeResponse::Streaming(_)));
}
