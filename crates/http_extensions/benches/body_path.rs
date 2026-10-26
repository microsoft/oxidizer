// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::missing_panics_doc, clippy::unwrap_used, reason = "benchmark code")]
#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{array, mem};

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample_with_inputs;
use bytes::Bytes;
use bytesbuf::BytesView;
use bytesbuf::mem::testing::TransparentMemory;
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use http::{Request, Uri};
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use http_extensions::{HttpBody, HttpBodyBuilder, HttpBodyOptions, HttpError, Result};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const REQUEST_BYTES: usize = 10_240;
const RESPONSE_BYTES: usize = 9_216;
const TARGET_RESPONSE_FRAMES: usize = 3;
const TARGET_RESPONSE_FRAME_BYTES: usize = RESPONSE_BYTES / TARGET_RESPONSE_FRAMES;
static RESPONSE_PAYLOAD: [u8; RESPONSE_BYTES] = [0; RESPONSE_BYTES];

fn prepared_inputs<T>(iters: u64, mut setup: impl FnMut() -> T) -> Vec<T> {
    (0..iters).map(|_| setup()).collect()
}

fn owned_frame(frame_bytes: usize) -> Bytes {
    Bytes::from_static(&RESPONSE_PAYLOAD[..frame_bytes])
}

fn response_body<const FRAME_COUNT: usize>(builder: &HttpBodyBuilder, frame_bytes: usize) -> HttpBody {
    let frames: [_; FRAME_COUNT] = array::from_fn(|_| BytesView::from(owned_frame(frame_bytes)));
    builder.body(FrameBody::new(frames), &HttpBodyOptions::default())
}

#[expect(clippy::too_many_lines, reason = "the benchmark keeps the complete body-path matrix together")]
fn entry(c: &mut Criterion) {
    let session = Session::new();
    let memory = TransparentMemory::new();
    let body_builder = HttpBodyBuilder::with_custom_memory(memory.clone(), &Clock::new_frozen());
    let request_payload = BytesView::copied_from_slice(&[0_u8; REQUEST_BYTES], &memory);
    let uri = Uri::from_static("http://127.0.0.1/request-response");

    let mut group = c.benchmark_group("body_path_request");

    let operation = session.operation("request_construction");
    group.bench_function("request_construction", |b| {
        b.iter_custom(|iters| {
            let payloads = prepared_inputs(iters, || request_payload.clone());
            time_sample_with_inputs(
                payloads,
                |sample_iters| operation.measure_thread().iterations(sample_iters),
                |payload| Request::post(uri.clone()).body(body_builder.bytes(mem::take(payload))).unwrap(),
            )
        });
    });

    let operation = session.operation("request_body_framing");
    group.bench_function("request_body_framing", |b| {
        b.iter_custom(|iters| {
            let bodies = prepared_inputs(iters, || body_builder.bytes(request_payload.clone()));
            time_sample_with_inputs(
                bodies,
                |sample_iters| operation.measure_thread().iterations(sample_iters),
                |body| block_on(body.frame()).unwrap().unwrap().into_data().unwrap(),
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("body_path_response");

    macro_rules! bench_handoff {
        ($name:literal, $operation_name:literal, $frame_count:literal, $frame_bytes:expr) => {
            let operation = session.operation($operation_name);
            group.bench_function($name, |b| {
                b.iter_custom(|iters| {
                    let frames = prepared_inputs(iters, || {
                        Some(array::from_fn::<_, $frame_count, _>(|_| owned_frame($frame_bytes)))
                    });
                    time_sample_with_inputs(
                        frames,
                        |sample_iters| operation.measure_thread().iterations(sample_iters),
                        |frames| frames.take().unwrap().map(BytesView::from),
                    )
                });
            });
        };
    }

    bench_handoff!("hyper_frame_handoff_1x9216", "hyper_frame_handoff_1x9216", 1, RESPONSE_BYTES);
    bench_handoff!(
        "hyper_frame_handoff_3x3072",
        "hyper_frame_handoff_3x3072",
        3,
        TARGET_RESPONSE_FRAME_BYTES
    );
    bench_handoff!("hyper_frame_handoff_9x1024", "hyper_frame_handoff_9x1024", 9, RESPONSE_BYTES / 9);

    let operation = session.operation("response_frame_consumption");
    group.bench_function("response_frame_consumption", |b| {
        b.iter_custom(|iters| {
            let bodies = prepared_inputs(iters, || {
                response_body::<TARGET_RESPONSE_FRAMES>(&body_builder, TARGET_RESPONSE_FRAME_BYTES)
            });
            time_sample_with_inputs(
                bodies,
                |sample_iters| operation.measure_thread().iterations(sample_iters),
                |body| {
                    let mut total = 0;
                    while let Some(frame) = block_on(body.frame()) {
                        total += frame.unwrap().into_data().unwrap().len();
                    }
                    total
                },
            )
        });
    });

    macro_rules! bench_response_drain {
        ($name:literal, $operation_name:literal, $frame_count:literal, $frame_bytes:expr) => {
            let operation = session.operation($operation_name);
            group.bench_function($name, |b| {
                b.iter_custom(|iters| {
                    let bodies = prepared_inputs(iters, || response_body::<$frame_count>(&body_builder, $frame_bytes));
                    time_sample_with_inputs(
                        bodies,
                        |sample_iters| operation.measure_thread().iterations(sample_iters),
                        |body| block_on(mem::replace(body, body_builder.empty()).into_bytes()).unwrap(),
                    )
                });
            });
        };
    }

    bench_response_drain!(
        "complete_response_drain_1x9216",
        "complete_response_drain_1x9216",
        1,
        RESPONSE_BYTES
    );
    bench_response_drain!(
        "complete_response_drain_3x3072",
        "complete_response_drain_3x3072",
        3,
        TARGET_RESPONSE_FRAME_BYTES
    );
    bench_response_drain!(
        "complete_response_drain_9x1024",
        "complete_response_drain_9x1024",
        9,
        RESPONSE_BYTES / 9
    );

    group.finish();

    let mut group = c.benchmark_group("body_path_complete");

    let operation = session.operation("complete_request_response");
    group.bench_function("complete_request_response", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || CompletePathInput {
                request: Some(request_payload.clone()),
                response: Some([
                    owned_frame(TARGET_RESPONSE_FRAME_BYTES),
                    owned_frame(TARGET_RESPONSE_FRAME_BYTES),
                    owned_frame(TARGET_RESPONSE_FRAME_BYTES),
                ]),
            });
            time_sample_with_inputs(
                inputs,
                |sample_iters| operation.measure_thread().iterations(sample_iters),
                |input| {
                    let request = Request::post(uri.clone())
                        .body(body_builder.bytes(input.request.take().unwrap()))
                        .unwrap();
                    let mut request_body = request.into_body();
                    _ = block_on(request_body.frame()).unwrap().unwrap();

                    let frames = input.response.take().unwrap().map(BytesView::from);
                    let response = body_builder.body(FrameBody::new(frames), &HttpBodyOptions::default());
                    block_on(response.into_bytes()).unwrap()
                },
            )
        });
    });

    group.finish();
}

struct CompletePathInput {
    request: Option<BytesView>,
    response: Option<[Bytes; 3]>,
}

#[derive(Debug)]
struct FrameBody<const FRAME_COUNT: usize> {
    frames: [Option<BytesView>; FRAME_COUNT],
    next: usize,
}

impl<const FRAME_COUNT: usize> FrameBody<FRAME_COUNT> {
    fn new(frames: [BytesView; FRAME_COUNT]) -> Self {
        Self {
            frames: frames.map(Some),
            next: 0,
        }
    }
}

impl<const FRAME_COUNT: usize> Body for FrameBody<FRAME_COUNT> {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        let next = self.next;
        let frame = self.frames.get_mut(next).and_then(Option::take).map(Frame::data);
        self.next += usize::from(frame.is_some());
        Poll::Ready(frame.map(Ok))
    }

    fn is_end_stream(&self) -> bool {
        self.next == self.frames.len()
    }

    fn size_hint(&self) -> SizeHint {
        let remaining = self.frames[self.next..]
            .iter()
            .filter_map(Option::as_ref)
            .map(BytesView::len)
            .sum::<usize>();
        SizeHint::with_exact(remaining as u64)
    }
}

criterion_group!(benches, entry);
criterion_main!(benches);
