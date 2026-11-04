// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The overhead that the server middleware adds to one request.
//!
//! Each stack gets one request before the measurement. At the first request of a sink on a thread,
//! `observed` allocates storage for that thread. The allocation count of `metabench` covers only
//! the first call of a case, so without that request it also counts this storage.

#[path = "../tests/common/mod.rs"]
mod common;

use std::borrow::Borrow;
use std::hint::black_box;
use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use common::{ACTIVE, Echoing, Failing, NeverResponding, Responding, Step, Tenant};
use criterion::{BatchSize, BenchmarkId, Criterion};
use data_privacy::RedactionEngine;
use http::{Method, StatusCode, Version};
use http_extensions::{HttpError, HttpRequest, HttpResponse, RequestHandler};
use http_telemetry::Telemetry;
use layered::Layer;
use observed::context::Transfer;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{FlushError, Sink, SinkId};
use tick::SimpleClock;

const GROUP: &str = "http_telemetry_server/request";
const CASE: &str = "get";

/// Reads every field and every enrichment of each event whose name `interested` accepts, and keeps
/// nothing. Thus the measurements do not include the work of keeping the events.
struct ReadAll {
    engine: RedactionEngine,
    interested: fn(&str) -> bool,
}

impl EventProcessor for ReadAll {
    fn is_interested(&self, description: &EventDescription) -> bool {
        (self.interested)(description.name())
    }

    fn process(&self, event: &EventView<'_>) {
        let mut read = |_: &FieldDescriptor, value: &FieldValueFn<'_>| {
            black_box(value(&self.engine));
            ControlFlow::Continue(())
        };
        let _ = event.visit_fields(&mut read);
        let _ = event.visit_enrichments(&mut read);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink with `count` processors that each read every field of every event.
fn reading_sink(count: usize) -> Sink {
    Sink::builder(SinkId::new("reading"))
        .clock(SimpleClock::new_frozen())
        .processors(
            (0..count)
                .map(|_| {
                    Arc::new(ReadAll {
                        engine: RedactionEngine::default(),
                        interested: |_| true,
                    }) as Arc<dyn EventProcessor>
                })
                .collect(),
        )
        .build()
}

/// A sink with one processor that reads every field of each event whose name `interested`
/// accepts.
fn filtered_sink(interested: fn(&str) -> bool) -> Sink {
    Sink::builder(SinkId::new("filtered"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(ReadAll {
            engine: RedactionEngine::default(),
            interested,
        })])
        .build()
}

/// Receives every event and reads none of its fields.
struct Interested;

impl EventProcessor for Interested {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, _: &EventView<'_>) {}

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink whose one processor receives every event and reads none of its fields.
fn interested_sink() -> Sink {
    Sink::builder(SinkId::new("interested"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(Interested)])
        .build()
}

/// A `PURGE` request, which `http.request.method` records as `_OTHER`.
fn purge() -> HttpRequest {
    common::request(
        Method::from_bytes(b"PURGE").expect("PURGE is a valid method name"),
        Version::HTTP_11,
    )
}

/// A request with a method that `http` has no constant for, and that is longer than 15 bytes.
/// `http` keeps such a method on the heap.
fn long_extension_method() -> HttpRequest {
    let method = Method::from_bytes(b"LONG-EXTENSION-METHOD").expect("LONG-EXTENSION-METHOD is a valid method name");
    common::request(method, Version::HTTP_11)
}

/// Sends `request` to `handler`, which responds or returns an error at the first poll.
fn send(handler: &impl RequestHandler, request: HttpRequest) -> Result<HttpResponse, HttpError> {
    // Pin on the stack, because `Box::pin` allocates and so changes the measurement.
    let future = pin!(handler.execute(request));
    let Poll::Ready(result) = future.poll(&mut Context::from_waker(Waker::noop())) else {
        unreachable!("the handler finishes at the first poll, and the layer does not wait");
    };
    result
}

/// Sends `request` to `handler`, which never responds. Then polls the future once and drops it.
fn abandon(handler: &impl RequestHandler, request: HttpRequest) {
    // Pin on the stack, because `Box::pin` allocates and so changes the measurement.
    let future = pin!(handler.execute(request));
    assert!(
        future.poll(&mut Context::from_waker(Waker::noop())).is_pending(),
        "the handler never responds"
    );
}

/// The server middleware over `sink`, in front of [`Responding`]. The stack already got one
/// request.
fn responding_stack(sink: Sink) -> Telemetry<Responding> {
    let stack = Telemetry::server(sink).layer(common::responding(StatusCode::OK));
    drop(send(&stack, common::get()));
    stack
}

/// The server middleware over `sink`, in front of [`NeverResponding`]. The stack already got
/// one request.
fn never_responding_stack(sink: Sink) -> Telemetry<NeverResponding> {
    let stack = Telemetry::server(sink).layer(NeverResponding);
    abandon(&stack, common::get());
    stack
}

/// The server middleware in front of `handler`, over a sink with two processors that read every
/// field, so the allocation count shows that the error event builds `error.type` once. The stack
/// already got one request.
fn failing_stack(handler: Failing) -> Telemetry<Failing> {
    let stack = Telemetry::server(reading_sink(2)).layer(handler);
    drop(send(&stack, common::get()));
    stack
}

/// The server middleware over `sink`, in front of [`Echoing`]. The stack already got one request.
fn echoing_stack(sink: Sink) -> Telemetry<Echoing> {
    let stack = Telemetry::server(sink).layer(Echoing(StatusCode::OK));
    drop(send(&stack, common::get()));
    stack
}

/// The server middleware with the excluded path `/books`, over a sink with one processor
/// that reads every field, in front of [`Responding`]. The stack already got one request.
fn excluded_stack() -> Telemetry<Responding> {
    let stack = Telemetry::server(reading_sink(1))
        .excluded_paths(["/books"])
        .layer(common::responding(StatusCode::OK));
    drop(send(&stack, common::get()));
    stack
}

/// A `GET` request over HTTP/1.1 whose body takes `steps`. [`Echoing`] responds with this body,
/// so the benchmark builds the response body before the measurement.
fn streamed(steps: impl IntoIterator<Item = Step>) -> HttpRequest {
    common::get().map(|_| common::scripted(steps))
}

/// Sends `request` to `handler`, polls the response body `polls` times, and drops it.
fn read_body(handler: &impl RequestHandler, request: HttpRequest, polls: usize) {
    let mut body = send(handler, request).expect("the handler responds").into_body();
    for _ in 0..polls {
        drop(black_box(common::poll_frame(&mut body)));
    }
}

/// The server middleware in front of [`Responding`], over a sink that combines three sinks, and
/// an enrichment for that sink. The stack already got one request.
fn enriched_stack() -> (Telemetry<Responding>, Transfer) {
    let sink = Sink::composite([reading_sink(1), reading_sink(1), reading_sink(1)]);
    let enrichment = sink.transfer_context().with_enrichment(Tenant { tenant: "contoso" });
    (responding_stack(sink), enrichment)
}

#[metabench::benchmark(INNER_ONLY, GROUP, "inner_only")]
#[bench::get(Box::new(common::responding(StatusCode::OK)), common::get())]
fn inner_only<S: Borrow<Responding>>(handler: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(handler.borrow(), request), handler)
}

#[metabench::benchmark(UNINTERESTED_SINK, GROUP, "uninterested_sink")]
#[bench::get(Box::new(responding_stack(Sink::noop())), common::get())]
fn uninterested_sink<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE_EMPTY_BODY, GROUP, "complete_empty_body")]
#[bench::get(Box::new(responding_stack(reading_sink(1))), common::get())]
fn complete_empty_body<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(ABANDONED, GROUP, "abandoned")]
#[bench::get(Box::new(never_responding_stack(reading_sink(1))), common::get())]
fn abandoned<S: Borrow<Telemetry<NeverResponding>>>(stack: S, request: HttpRequest) -> S {
    abandon(stack.borrow(), request);
    stack
}

#[metabench::benchmark(METHOD_ORIGINAL_READ, GROUP, "method_original_read")]
#[bench::purge(Box::new(responding_stack(reading_sink(2))), purge())]
fn method_original_read<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(LONG_METHOD, GROUP, "long_method")]
#[bench::long_extension_method(Box::new(responding_stack(interested_sink())), long_extension_method())]
fn long_method<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(ENRICHED_THREE_SINKS, GROUP, "enriched_three_sinks")]
#[bench::get(Box::new(enriched_stack()), common::get())]
fn enriched_three_sinks<S: Borrow<(Telemetry<Responding>, Transfer)>>(
    setup: S,
    request: HttpRequest,
) -> (Result<HttpResponse, HttpError>, S) {
    let response = {
        let (stack, enrichment) = setup.borrow();
        let _enrichment = enrichment.apply_current_thread();
        send(stack, request)
    };
    (response, setup)
}

#[metabench::benchmark(ACTIVE_REQUESTS_ONLY, GROUP, "active_requests_only")]
#[bench::https(
    Box::new(responding_stack(filtered_sink(|name| name == ACTIVE))),
    common::get_to("https://example.com/books/1")
)]
#[bench::ftp(
    Box::new(responding_stack(filtered_sink(|name| name == ACTIVE))),
    common::get_to("ftp://example.com/books/1")
)]
fn active_requests_only<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(REQUEST_EVENTS_ONLY, GROUP, "request_events_only")]
#[bench::get(Box::new(responding_stack(filtered_sink(|name| name != ACTIVE))), common::get())]
fn request_events_only<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE_WITH_SERVER_ADDRESS, GROUP, "complete_with_server_address")]
#[bench::get(Box::new(responding_stack(reading_sink(2))), common::get_to("http://example.com/books/1"))]
fn complete_with_server_address<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(EXCLUDED_PATH, GROUP, "excluded_path")]
#[bench::get(Box::new(excluded_stack()), common::get())]
fn excluded_path<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE_STREAMED_BODY, GROUP, "complete_streamed_body")]
#[bench::get(Box::new(echoing_stack(reading_sink(1))), streamed([common::data("chunk"), Step::End]))]
fn complete_streamed_body<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> S {
    read_body(stack.borrow(), request, 2);
    stack
}

#[metabench::benchmark(ABANDONED_BODY, GROUP, "abandoned_body")]
#[bench::get(Box::new(echoing_stack(reading_sink(1))), streamed([common::data("chunk"), Step::Pending]))]
fn abandoned_body<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> S {
    read_body(stack.borrow(), request, 1);
    stack
}

#[metabench::benchmark(BODY_ERROR, GROUP, "body_error")]
#[bench::get(
    Box::new(echoing_stack(reading_sink(1))),
    streamed([Step::Error(HttpError::validation("the page is missing"))])
)]
fn body_error<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> S {
    read_body(stack.borrow(), request, 1);
    stack
}

#[metabench::benchmark(INNER_ERROR, GROUP, "inner_error")]
#[bench::validation(Box::new(common::FAILING), common::get())]
#[bench::connection_refused(Box::new(common::FAILING_CHAINED), common::get())]
fn inner_error<S: Borrow<Failing>>(handler: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(handler.borrow(), request), handler)
}

#[metabench::benchmark(HANDLER_ERROR, GROUP, "handler_error")]
#[bench::validation(Box::new(failing_stack(common::FAILING)), common::get())]
#[bench::connection_refused(Box::new(failing_stack(common::FAILING_CHAINED)), common::get())]
fn handler_error<S: Borrow<Telemetry<Failing>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(GROUP);
    let handler = common::responding(StatusCode::OK);
    group.bench_function(BenchmarkId::new(INNER_ONLY.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(common::get, |request| inner_only(&handler, request).0, BatchSize::SmallInput);
    });
    let stack = responding_stack(Sink::noop());
    group.bench_function(BenchmarkId::new(UNINTERESTED_SINK.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(common::get, |request| uninterested_sink(&stack, request).0, BatchSize::SmallInput);
    });
    let stack = responding_stack(reading_sink(1));
    group.bench_function(BenchmarkId::new(COMPLETE_EMPTY_BODY.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(common::get, |request| complete_empty_body(&stack, request).0, BatchSize::SmallInput);
    });
    let stack = never_responding_stack(reading_sink(1));
    group.bench_function(BenchmarkId::new(ABANDONED.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(common::get, |request| abandoned(&stack, request), BatchSize::SmallInput);
    });
    let stack = responding_stack(reading_sink(2));
    group.bench_function(BenchmarkId::new(METHOD_ORIGINAL_READ.benchmark_name(), "purge"), |bencher| {
        bencher.iter_batched(purge, |request| method_original_read(&stack, request).0, BatchSize::SmallInput);
    });
    let stack = responding_stack(interested_sink());
    group.bench_function(BenchmarkId::new(LONG_METHOD.benchmark_name(), "long_extension_method"), |bencher| {
        bencher.iter_batched(
            long_extension_method,
            |request| long_method(&stack, request).0,
            BatchSize::SmallInput,
        );
    });
    let setup = enriched_stack();
    group.bench_function(BenchmarkId::new(ENRICHED_THREE_SINKS.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(
            common::get,
            |request| enriched_three_sinks(&setup, request).0,
            BatchSize::SmallInput,
        );
    });
    let stack = responding_stack(filtered_sink(|name| name == ACTIVE));
    for (case, uri) in [("https", "https://example.com/books/1"), ("ftp", "ftp://example.com/books/1")] {
        group.bench_function(BenchmarkId::new(ACTIVE_REQUESTS_ONLY.benchmark_name(), case), |bencher| {
            bencher.iter_batched(
                || common::get_to(uri),
                |request| active_requests_only(&stack, request).0,
                BatchSize::SmallInput,
            );
        });
    }
    let stack = responding_stack(filtered_sink(|name| name != ACTIVE));
    group.bench_function(BenchmarkId::new(REQUEST_EVENTS_ONLY.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(common::get, |request| request_events_only(&stack, request).0, BatchSize::SmallInput);
    });
    let stack = responding_stack(reading_sink(2));
    group.bench_function(BenchmarkId::new(COMPLETE_WITH_SERVER_ADDRESS.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(
            || common::get_to("http://example.com/books/1"),
            |request| complete_with_server_address(&stack, request).0,
            BatchSize::SmallInput,
        );
    });
    let stack = excluded_stack();
    group.bench_function(BenchmarkId::new(EXCLUDED_PATH.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(common::get, |request| excluded_path(&stack, request).0, BatchSize::SmallInput);
    });
    let stack = echoing_stack(reading_sink(1));
    group.bench_function(BenchmarkId::new(COMPLETE_STREAMED_BODY.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(
            || streamed([common::data("chunk"), Step::End]),
            |request| complete_streamed_body(&stack, request),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new(ABANDONED_BODY.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(
            || streamed([common::data("chunk"), Step::Pending]),
            |request| abandoned_body(&stack, request),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new(BODY_ERROR.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(
            || streamed([Step::Error(HttpError::validation("the page is missing"))]),
            |request| body_error(&stack, request),
            BatchSize::SmallInput,
        );
    });
    for (case, handler) in [("validation", common::FAILING), ("connection_refused", common::FAILING_CHAINED)] {
        group.bench_function(BenchmarkId::new(INNER_ERROR.benchmark_name(), case), |bencher| {
            bencher.iter_batched(common::get, |request| inner_error(&handler, request).0, BatchSize::SmallInput);
        });
        let stack = failing_stack(handler);
        group.bench_function(BenchmarkId::new(HANDLER_ERROR.benchmark_name(), case), |bencher| {
            bencher.iter_batched(common::get, |request| handler_error(&stack, request).0, BatchSize::SmallInput);
        });
    }
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        INNER_ONLY,
        UNINTERESTED_SINK,
        COMPLETE_EMPTY_BODY,
        ABANDONED,
        METHOD_ORIGINAL_READ,
        LONG_METHOD,
        ENRICHED_THREE_SINKS,
        ACTIVE_REQUESTS_ONLY,
        REQUEST_EVENTS_ONLY,
        COMPLETE_WITH_SERVER_ADDRESS,
        EXCLUDED_PATH,
        COMPLETE_STREAMED_BODY,
        ABANDONED_BODY,
        BODY_ERROR,
        INNER_ERROR,
        HANDLER_ERROR,
    ],
);
