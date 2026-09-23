// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated unary request transcoding and fully consumed streaming responses.

#![allow(missing_docs, reason = "benchmark code has no public API")]
#![allow(clippy::unwrap_used, clippy::panic, reason = "invalid benchmark fixtures must fail the run")]
#![allow(clippy::needless_pass_by_value, reason = "Gungraun owns prepared inputs")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "metabench emits Gungraun entry points"
)]

use std::convert::Infallible;
use std::hint::black_box;

use bytes::Bytes;
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use futures::StreamExt as _;
use futures::executor::block_on;
use http::{HeaderMap, HeaderValue};
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full, StreamBody};
use rest_over_grpc::serving::serve_http;
use rest_over_grpc::transcoding::{Transcode as _, TranscodeResponse};
use rest_over_grpc_tests::custom::{InMemoryLibrary, Transcoder};

const REQUEST: &str = "rog_transcode/request";
const STREAM: &str = "rog_transcode/stream";
const ADAPTER: &str = "rog_transcode/adapter";

struct Input {
    transcoder: Transcoder<InMemoryLibrary>,
    method: &'static str,
    path: String,
    body: &'static [u8],
    accept: Option<&'static str>,
}

fn input(method: &'static str, path: &'static str, body: &'static [u8], accept: Option<&'static str>) -> Input {
    Input {
        transcoder: Transcoder::new(InMemoryLibrary),
        method,
        path: path.to_owned(),
        body,
        accept,
    }
}

fn query_input(matched: bool, count: usize) -> Input {
    let mut input = input("GET", "/v1/nope", b"", None);
    let route = if matched { "/v1/shelves:search" } else { "/v1/nope" };
    input.path = format!("{route}?{}", vec!["tags=one"; count].join("&"));
    input
}

fn response(input: Input) -> TranscodeResponse {
    let mut headers = HeaderMap::new();
    if let Some(accept) = input.accept {
        headers.insert("accept", HeaderValue::from_static(accept));
    }
    block_on(
        input
            .transcoder
            .transcode(black_box(input.method), black_box(&input.path), headers, black_box(input.body)),
    )
}

fn consume_stream(input: Input) -> Vec<u8> {
    let response = response(input);
    match response {
        TranscodeResponse::Streaming(stream) => {
            let frames: Vec<Vec<u8>> = block_on(stream.into_frames().map(|frame| frame.unwrap()).collect());
            frames.concat()
        }
        TranscodeResponse::Unary(_) => panic!("stream route must yield streaming response"),
    }
}

struct AdapterInput {
    transcoder: Transcoder<InMemoryLibrary>,
    request: http::Request<UnsyncBoxBody<Bytes, Infallible>>,
}

fn adapter_input(case: &str) -> AdapterInput {
    let (method, path, body, accept, split) = match case {
        "one_frame" => (
            "POST",
            "/v1/shelves",
            br#"{"name":"ignored","theme":"sci-fi"}"#.as_slice(),
            None,
            false,
        ),
        "multi_frame" => (
            "POST",
            "/v1/shelves",
            br#"{"name":"ignored","theme":"sci-fi"}"#.as_slice(),
            None,
            true,
        ),
        "matched_get" => ("GET", "/v1/shelves/history", b"".as_slice(), None, false),
        "unmatched" | "unmatched_query_64" => ("GET", "/v1/nope", b"".as_slice(), None, false),
        "matched_query_64" => ("GET", "/v1/shelves:search", b"".as_slice(), None, false),
        "stream_json" => ("GET", "/v1/shelves:stream", b"".as_slice(), None, false),
        "stream_ndjson" => ("GET", "/v1/shelves:stream", b"".as_slice(), Some("application/x-ndjson"), false),
        "stream_sse" => ("GET", "/v1/shelves:stream", b"".as_slice(), Some("text/event-stream"), false),
        _ => panic!("unknown adapter fixture"),
    };
    let frames = [body[..body.len() / 2].to_vec(), body[body.len() / 2..].to_vec()]
        .into_iter()
        .map(|frame| Ok::<_, Infallible>(Frame::data(Bytes::from(frame))));
    let boxed = if split {
        StreamBody::new(futures::stream::iter(frames)).boxed_unsync()
    } else {
        Full::new(Bytes::copy_from_slice(body)).boxed_unsync()
    };
    let target = if case.ends_with("_query_64") {
        format!("{path}?{}", vec!["tags=one"; 64].join("&"))
    } else {
        path.to_owned()
    };
    let mut builder = http::Request::builder().method(method).uri(target);
    if let Some(accept) = accept {
        builder = builder.header("accept", accept);
    }
    AdapterInput {
        transcoder: Transcoder::new(InMemoryLibrary),
        request: builder.body(boxed).unwrap(),
    }
}

#[metabench::benchmark(SERVED, ADAPTER, "serve", gungraun_setup = verified_adapter)]
#[bench::one_frame(adapter_input("one_frame"))]
#[bench::multi_frame(adapter_input("multi_frame"))]
#[bench::matched_get(adapter_input("matched_get"))]
#[bench::unmatched(adapter_input("unmatched"))]
#[bench::stream_json(adapter_input("stream_json"))]
#[bench::stream_ndjson(adapter_input("stream_ndjson"))]
#[bench::stream_sse(adapter_input("stream_sse"))]
fn serve(input: AdapterInput) -> (u16, Vec<u8>) {
    block_on(async {
        let response = serve_http(black_box(input.request), black_box(&input.transcoder)).await;
        let status = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
        black_box((status, body))
    })
}

#[metabench::benchmark(UNARY, REQUEST, "unary", gungraun_setup = verified_input)]
#[bench::path(input("GET", "/v1/shelves/history", b"", None))]
#[bench::body(input("POST", "/v1/shelves", br#"{"name":"ignored","theme":"sci-fi"}"#, None))]
#[bench::flat_query(input("GET", "/v1/shelves?filter=science", b"", None))]
#[bench::dotted_query(input("GET", "/v1/shelves:search?options%2Eenabled=true&tags=one&tags=two", b"", None))]
#[bench::body_overlay(input(
    "POST",
    "/v1/shelves/42:replace?force=true",
    br#"{"shelf":{"name":"ignored","theme":"history"}}"#,
    None
))]
#[bench::miss(input("GET", "/v1/nope", b"", None))]
#[bench::matched_query_8(query_input(true, 8))]
#[bench::matched_query_64(query_input(true, 64))]
#[bench::unmatched_query_8(query_input(false, 8))]
#[bench::unmatched_query_64(query_input(false, 64))]
fn transcode_unary(input: Input) -> TranscodeResponse {
    black_box(response(input))
}

#[metabench::benchmark(STREAMED, STREAM, "frames", gungraun_setup = verified_input)]
#[bench::json(input("GET", "/v1/shelves:stream", b"", None))]
#[bench::ndjson(input("GET", "/v1/shelves:stream", b"", Some("application/x-ndjson")))]
#[bench::sse(input("GET", "/v1/shelves:stream", b"", Some("text/event-stream")))]
fn transcode_stream(input: Input) -> Vec<u8> {
    black_box(consume_stream(input))
}

fn verified_input(input: Input) -> Input {
    verify();
    input
}

fn verified_adapter(input: AdapterInput) -> AdapterInput {
    verify();
    input
}

fn verify() {
    let cases = [
        (input("GET", "/v1/shelves/history", b"", None), 200, Some("shelves/history")),
        (
            input("POST", "/v1/shelves", br#"{"name":"ignored","theme":"sci-fi"}"#, None),
            200,
            Some("shelves/created"),
        ),
        (input("GET", "/v1/shelves?filter=science", b"", None), 200, Some("science")),
        (
            input("GET", "/v1/shelves:search?options%2Eenabled=true&tags=one&tags=two", b"", None),
            200,
            Some("enabled"),
        ),
        (
            input(
                "POST",
                "/v1/shelves/42:replace?force=true",
                br#"{"shelf":{"name":"ignored","theme":"history"}}"#,
                None,
            ),
            200,
            Some("1"),
        ),
        (input("GET", "/v1/nope", b"", None), 404, None),
        (query_input(true, 8), 200, None),
        (query_input(true, 64), 200, None),
        (query_input(false, 8), 404, None),
        (query_input(false, 64), 404, None),
    ];
    for (input, status, fragment) in cases {
        let TranscodeResponse::Unary(http) = response(input) else {
            panic!("unary fixture returned a stream");
        };
        assert_eq!(http.status().as_u16(), status);
        if let Some(fragment) = fragment {
            assert!(String::from_utf8_lossy(http.body()).contains(fragment));
        }
    }
    for (accept, prefix, suffix) in [
        (None, b"[".as_slice(), b"]".as_slice()),
        (Some("application/x-ndjson"), b"{".as_slice(), b"\n".as_slice()),
        (Some("text/event-stream"), b"data: {".as_slice(), b"\n\n".as_slice()),
    ] {
        let body = consume_stream(input("GET", "/v1/shelves:stream", b"", accept));
        assert!(body.starts_with(prefix) && body.ends_with(suffix));
        assert_eq!(body.windows(b"shelves/".len()).filter(|chunk| *chunk == b"shelves/").count(), 2);
    }
    for (case, status, fragment) in [
        ("one_frame", 200, "shelves/created"),
        ("multi_frame", 200, "shelves/created"),
        ("matched_get", 200, "shelves/history"),
        ("unmatched", 404, ""),
        ("matched_query_64", 200, ""),
        ("unmatched_query_64", 404, ""),
        ("stream_json", 200, "shelves/1"),
        ("stream_ndjson", 200, "shelves/1"),
        ("stream_sse", 200, "shelves/1"),
    ] {
        let (actual_status, body) = serve(adapter_input(case));
        assert_eq!(actual_status, status, "{case}");
        assert!(String::from_utf8_lossy(&body).contains(fragment), "{case}");
    }
}

fn criterion_benchmarks(c: &mut Criterion) {
    verify();
    let mut unary = c.benchmark_group(UNARY.group_name());
    for (case, method, path, body) in [
        ("path", "GET", "/v1/shelves/history", b"".as_slice()),
        ("body", "POST", "/v1/shelves", br#"{"name":"ignored","theme":"sci-fi"}"#.as_slice()),
        ("flat_query", "GET", "/v1/shelves?filter=science", b"".as_slice()),
        (
            "dotted_query",
            "GET",
            "/v1/shelves:search?options%2Eenabled=true&tags=one&tags=two",
            b"".as_slice(),
        ),
        (
            "body_overlay",
            "POST",
            "/v1/shelves/42:replace?force=true",
            br#"{"shelf":{"name":"ignored","theme":"history"}}"#.as_slice(),
        ),
        ("miss", "GET", "/v1/nope", b"".as_slice()),
    ] {
        unary.throughput(Throughput::Bytes(body.len().max(1) as u64));
        unary.bench_function(BenchmarkId::new(UNARY.benchmark_name(), case), |b| {
            b.iter_batched(|| input(method, path, body, None), transcode_unary, BatchSize::SmallInput);
        });
    }
    for matched in [true, false] {
        for count in [8, 64] {
            let case = format!("{}_query_{count}", if matched { "matched" } else { "unmatched" });
            unary.bench_function(BenchmarkId::new(UNARY.benchmark_name(), case), |b| {
                b.iter_batched(|| query_input(matched, count), transcode_unary, BatchSize::SmallInput);
            });
        }
    }
    unary.finish();
    let mut streamed = c.benchmark_group(STREAMED.group_name());
    for (case, accept) in [
        ("json", None),
        ("ndjson", Some("application/x-ndjson")),
        ("sse", Some("text/event-stream")),
    ] {
        streamed.throughput(Throughput::Elements(2));
        streamed.bench_function(BenchmarkId::new(STREAMED.benchmark_name(), case), |b| {
            b.iter_batched(
                || input("GET", "/v1/shelves:stream", b"", accept),
                transcode_stream,
                BatchSize::SmallInput,
            );
        });
    }
    streamed.finish();
    let mut adapter = c.benchmark_group(SERVED.group_name());
    for case in [
        "one_frame",
        "multi_frame",
        "matched_get",
        "unmatched",
        "matched_query_64",
        "unmatched_query_64",
        "stream_json",
        "stream_ndjson",
        "stream_sse",
    ] {
        adapter.bench_function(BenchmarkId::new(SERVED.benchmark_name(), case), |b| {
            b.iter_batched(|| adapter_input(case), serve, BatchSize::SmallInput);
        });
    }
    adapter.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [UNARY, STREAMED, SERVED]);
