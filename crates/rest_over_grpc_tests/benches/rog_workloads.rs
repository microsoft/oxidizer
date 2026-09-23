// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated request overlays and streamed JSON/NDJSON/SSE framing.

#![allow(missing_docs, reason = "benchmark code has no public API")]
#![allow(clippy::unwrap_used, clippy::panic, reason = "invalid benchmark fixtures must fail the run")]
#![allow(clippy::needless_pass_by_value, reason = "Gungraun owns prepared inputs")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "metabench emits Gungraun entry points"
)]

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use futures::StreamExt as _;
use futures::executor::block_on;
use rest_over_grpc::codegen_helpers::{RequestBodyKind, StreamEncoding, decode_request, encode_frames};
use rest_over_grpc_tests::custom::pb::SearchShelvesRequest;
use serde::Serialize;
use serde_json::Value;

const OVERLAY: &str = "rog_workloads/overlay";
const FRAMING: &str = "rog_workloads/framing";

struct OverlayInput {
    body: &'static [u8],
    query: Vec<(&'static str, &'static str)>,
}

fn overlay_input(case: &str) -> OverlayInput {
    let (body, query) = match case {
        "body_only" => (br#"{"k0":"body","k1":"body"}"#.as_slice(), vec![]),
        "flat_small" => (br#"{"k0":"body","k1":"body"}"#.as_slice(), vec![("k0", "query")]),
        "flat_many" => (
            br#"{"k0":"body","k1":"body","k2":"body","k3":"body","k4":"body","k5":"body","k6":"body","k7":"body"}"#.as_slice(),
            vec![
                ("k0", "query"),
                ("k1", "query"),
                ("k2", "query"),
                ("k3", "query"),
                ("k4", "query"),
                ("k5", "query"),
                ("k6", "query"),
                ("k7", "query"),
            ],
        ),
        "dotted_small" => (br#"{"k0":"body"}"#.as_slice(), vec![("options.enabled", "true")]),
        "dotted_many" => (
            br#"{"k0":"body","k1":"body","k2":"body","k3":"body"}"#.as_slice(),
            vec![
                ("options.enabled", "true"),
                ("options.label", "science"),
                ("other.enabled", "false"),
                ("other.label", "history"),
            ],
        ),
        _ => panic!("unknown overlay fixture"),
    };
    OverlayInput { body, query }
}

#[metabench::benchmark(DECODE, OVERLAY, "decode", gungraun_setup = verified_overlay)]
#[bench::body_only(overlay_input("body_only"))]
#[bench::flat_small(overlay_input("flat_small"))]
#[bench::flat_many(overlay_input("flat_many"))]
#[bench::dotted_small(overlay_input("dotted_small"))]
#[bench::dotted_many(overlay_input("dotted_many"))]
fn decode(input: OverlayInput) -> Value {
    black_box(decode_request::<Value>(black_box(&input.query), black_box(input.body), RequestBodyKind::Whole).unwrap())
}

fn repeated_input() -> OverlayInput {
    OverlayInput {
        body: br#"{"tags":["body"],"includeArchived":true,"options":{"enabled":true}}"#,
        query: vec![("tags", "one"), ("tags", "two"), ("include_archived", "false")],
    }
}

#[metabench::benchmark(REPEATED, OVERLAY, "typed", gungraun_setup = verified_overlay)]
#[bench::repeated(repeated_input())]
fn decode_repeated(input: OverlayInput) -> SearchShelvesRequest {
    black_box(decode_request::<SearchShelvesRequest>(black_box(&input.query), black_box(input.body), RequestBodyKind::Whole).unwrap())
}

#[derive(Serialize)]
struct Payload {
    name: String,
}

struct FrameInput {
    items: Vec<Result<Payload, rest_over_grpc::handling::Status>>,
    encoding: StreamEncoding,
}

fn frame_input(encoding: StreamEncoding, count: usize, size: usize, spike: bool) -> FrameInput {
    let items = (0..count)
        .map(|i| {
            let len = if spike && i % 32 == 31 { 4096 } else { size };
            Ok(Payload { name: "x".repeat(len) })
        })
        .collect();
    FrameInput { items, encoding }
}

#[metabench::benchmark(FRAMES, FRAMING, "encode", gungraun_setup = verified_frame)]
#[bench::json_small_1(frame_input(StreamEncoding::JsonArray, 1, 16, false))]
#[bench::json_medium_128(frame_input(StreamEncoding::JsonArray, 128, 256, false))]
#[bench::json_large_2048(frame_input(StreamEncoding::JsonArray, 2048, 16, false))]
#[bench::json_spike_128(frame_input(StreamEncoding::JsonArray, 128, 16, true))]
#[bench::ndjson_small_1(frame_input(StreamEncoding::NdJson, 1, 16, false))]
#[bench::ndjson_medium_128(frame_input(StreamEncoding::NdJson, 128, 256, false))]
#[bench::ndjson_large_2048(frame_input(StreamEncoding::NdJson, 2048, 16, false))]
#[bench::ndjson_spike_128(frame_input(StreamEncoding::NdJson, 128, 16, true))]
#[bench::sse_small_1(frame_input(StreamEncoding::Sse, 1, 16, false))]
#[bench::sse_medium_128(frame_input(StreamEncoding::Sse, 128, 256, false))]
#[bench::sse_large_2048(frame_input(StreamEncoding::Sse, 2048, 16, false))]
#[bench::sse_spike_128(frame_input(StreamEncoding::Sse, 128, 16, true))]
fn encode(input: FrameInput) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = block_on(
        encode_frames(futures::stream::iter(input.items), black_box(input.encoding))
            .map(|frame| frame.unwrap())
            .collect(),
    );
    black_box(frames.concat())
}

fn verified_overlay(input: OverlayInput) -> OverlayInput {
    verify();
    input
}

fn verified_frame(input: FrameInput) -> FrameInput {
    verify();
    input
}

fn verify() {
    for case in ["body_only", "flat_small", "flat_many", "dotted_small", "dotted_many"] {
        let decoded = decode(overlay_input(case));
        assert!(decoded.is_object(), "{case}");
        if case.starts_with("flat") {
            assert_eq!(decoded["k0"], "query");
        }
        let repeated = decode_repeated(repeated_input());
        assert_eq!(repeated.tags, ["one", "two"]);
        assert!(!repeated.include_archived);
        if case.starts_with("dotted") {
            assert!(decoded["options"]["enabled"] == "true" || decoded["options"]["enabled"] == true);
        }
    }
    for encoding in [StreamEncoding::JsonArray, StreamEncoding::NdJson, StreamEncoding::Sse] {
        let body = encode(frame_input(encoding, 2, 16, false));
        let text = std::str::from_utf8(&body).unwrap();
        match encoding {
            StreamEncoding::JsonArray => assert_eq!(serde_json::from_slice::<Vec<Value>>(&body).unwrap().len(), 2),
            StreamEncoding::NdJson => assert_eq!(text.lines().count(), 2),
            StreamEncoding::Sse => assert_eq!(text.matches("data: ").count(), 2),
            _ => panic!("unexpected stream encoding"),
        }
        assert_eq!(text.matches(&"x".repeat(16)).count(), 2);
    }
}

fn criterion_benchmarks(c: &mut Criterion) {
    verify();
    let mut overlay = c.benchmark_group(DECODE.group_name());
    for case in ["body_only", "flat_small", "flat_many", "dotted_small", "dotted_many"] {
        overlay.bench_function(BenchmarkId::new(DECODE.benchmark_name(), case), |b| {
            b.iter_batched(|| overlay_input(case), decode, BatchSize::SmallInput);
        });
    }
    overlay.bench_function(BenchmarkId::new(REPEATED.benchmark_name(), "repeated"), |b| {
        b.iter_batched(repeated_input, decode_repeated, BatchSize::SmallInput);
    });
    overlay.finish();
    let mut framing = c.benchmark_group(FRAMES.group_name());
    for (name, encoding) in [
        ("json", StreamEncoding::JsonArray),
        ("ndjson", StreamEncoding::NdJson),
        ("sse", StreamEncoding::Sse),
    ] {
        for (shape, count, size, spike) in [
            ("small_1", 1, 16, false),
            ("medium_128", 128, 256, false),
            ("large_2048", 2048, 16, false),
            ("spike_128", 128, 16, true),
        ] {
            framing.throughput(Throughput::Elements(count as u64));
            framing.bench_function(BenchmarkId::new(FRAMES.benchmark_name(), format!("{name}_{shape}")), |b| {
                b.iter_batched(|| frame_input(encoding, count, size, spike), encode, BatchSize::SmallInput);
            });
        }
    }
    framing.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [DECODE, REPEATED, FRAMES]);
