// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Handling a client-streaming gRPC API (e.g. a chunked upload) alongside
//! `rest_over_grpc`.
//!
//! Client-streaming and bidirectional RPCs have no `google.api.http` mapping — an
//! HTTP request is a single message — so `rest_over_grpc::build` rejects them at
//! codegen time (*"method X is streaming, which cannot be transcoded to unary
//! REST"*). And even reframed as unary, the transcoder buffers the whole request
//! body as one JSON `&[u8]`, so it is unsuitable for large or binary uploads.
//!
//! The fix is to *not* transcode the upload: give it a dedicated HTTP handler
//! that reads the body incrementally and forwards each chunk to your native gRPC
//! client-streaming call (or straight to blob storage), and let `rest_over_grpc`
//! handle the surrounding JSON routes. This example shows that composition: a
//! tiny router sends `/v1/...` JSON routes through the generated `transcode`, and
//! a `POST /v1/videos:upload` route through a streaming handler that reads the
//! body one frame at a time and never touches `transcode`.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example client_streaming_upload
//! ```

use std::convert::Infallible;

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{Method, Request, Response, StatusCode};
use http_body::{Body, Frame};
use http_body_util::{BodyExt as _, Empty, StreamBody};
use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
use rest_over_grpc_examples::custom::{InMemoryLibrary, Transcoder};

/// A deliberately small cap; the point is that a streaming handler can enforce a
/// limit *before* buffering the whole body, which the transcoder cannot.
const MAX_UPLOAD: usize = 8 * 1024 * 1024;

/// Stand-in for the result of a native gRPC client-streaming handler. A real one
/// would consume a stream of decoded request messages (e.g.
/// `tonic::Streaming<VideoChunk>`) and return a single response; here it just
/// tallies what it received.
#[derive(Default)]
struct UploadSummary {
    chunks: usize,
    bytes: usize,
}

/// Reads an HTTP body one frame at a time, forwarding each chunk onward and never
/// holding more than a single chunk in memory. This is what replaces the
/// (impossible) transcoded client-streaming RPC.
async fn handle_upload<B>(request: Request<B>) -> Response<Vec<u8>>
where
    B: Body<Data = Bytes> + Unpin,
{
    let mut body = request.into_body();
    let mut summary = UploadSummary::default();

    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return text(StatusCode::BAD_REQUEST, "error reading upload body");
        };
        let Ok(chunk) = frame.into_data() else {
            continue; // a trailers frame, not data
        };

        summary.bytes += chunk.len();
        if summary.bytes > MAX_UPLOAD {
            return text(StatusCode::PAYLOAD_TOO_LARGE, "upload too large");
        }
        summary.chunks += 1;

        // A real handler streams `chunk` onward here — a gRPC client-streaming
        // `send`, or a write to blob storage — with backpressure, then drops it.
    }

    json(
        StatusCode::OK,
        format!(r#"{{"chunks":{},"bytes":{}}}"#, summary.chunks, summary.bytes).into_bytes(),
    )
}

/// The outer router: the upload route goes through the streaming handler; every
/// other route is transcoded through the generated `transcode`.
async fn route<B>(library: &Transcoder<InMemoryLibrary>, request: Request<B>) -> Response<Vec<u8>>
where
    B: Body<Data = Bytes> + Unpin,
{
    if request.method() == Method::POST && request.uri().path() == "/v1/videos:upload" {
        return handle_upload(request).await;
    }

    // Everything else is a small unary JSON request: collect the body, transcode,
    // and buffer the reply.
    let method = request.method().clone();
    let target = request.uri().path_and_query().map_or("/", |pq| pq.as_str()).to_owned();
    let headers = request.headers().clone();
    let bytes = request
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let TranscodeResponse::Unary(response) = library.transcode(method.as_str(), &target, headers, &bytes).await else {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "unexpected streaming response");
    };
    json(response.status(), response.into_body())
}

fn json(status: StatusCode, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .expect("response builds")
}

fn text(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain")
        .body(message.as_bytes().to_vec())
        .expect("response builds")
}

fn report(label: &str, response: &Response<Vec<u8>>) {
    println!("{label} -> {} {}", response.status(), String::from_utf8_lossy(response.body()));
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let library = Transcoder::new(InMemoryLibrary);

    // A normal JSON route, transcoded as usual.
    let get = Request::builder()
        .method(Method::GET)
        .uri("/v1/shelves/history")
        .body(Empty::<Bytes>::new())
        .expect("request builds");
    report("GET /v1/shelves/history", &route(&library, get).await);

    // A chunked upload — the client sends the "file" as several body frames,
    // which the streaming handler consumes one at a time.
    let chunks = [b"chunk-0-data".as_slice(), b"chunk-1-data", b"chunk-2-data"]
        .into_iter()
        .map(|bytes| Ok::<_, Infallible>(Frame::data(Bytes::from_static(bytes))));
    let upload = Request::builder()
        .method(Method::POST)
        .uri("/v1/videos:upload")
        .body(StreamBody::new(futures_util::stream::iter(chunks)))
        .expect("request builds");
    report("POST /v1/videos:upload", &route(&library, upload).await);
}
