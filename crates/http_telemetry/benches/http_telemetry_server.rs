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

use common::{ACTIVE, Echoing, Failing, NeverResponding, PUBLIC, Responding, SECRET, Step, Tenant};
use criterion::{BatchSize, BenchmarkId, Criterion};
use data_privacy::{DataClass, RedactionEngine};
use http::header::RETRY_AFTER;
use http::{HeaderName, HeaderValue, Method, Request, StatusCode, Version};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, RequestHandler};
use http_telemetry::Telemetry;
use layered::Layer;
use observed::context::Transfer;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{FlushError, Sink, SinkId};
use templated_uri::BasePath;
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

/// A sink with `count` processors that each read every field of every event. Their engine prints
/// the values of [`PUBLIC`] unchanged, and erases the values of every other data class.
fn reading_sink(count: usize) -> Sink {
    Sink::builder(SinkId::new("reading"))
        .clock(SimpleClock::new_frozen())
        .processors(
            (0..count)
                .map(|_| {
                    Arc::new(ReadAll {
                        engine: common::public_engine(),
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

/// The server middleware with the excluded path `/books/`, over a sink with one processor
/// that reads every field, in front of [`Responding`]. The stack already got one request.
fn excluded_stack() -> Telemetry<Responding> {
    let stack = Telemetry::server(reading_sink(1))
        .excluded_paths([BasePath::from_static("/books/")])
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

/// The routes of the resolver that the routed stacks use.
#[routerama::resolver]
#[expect(
    dead_code,
    reason = "the middleware reads the route table of the resolver, not the captured fields"
)]
enum Route<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
}

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The server middleware with a resolver for [`Route`], over a sink with two processors that read
/// every field, in front of [`Responding`]. If `class` is set, it is the data class of the route
/// parameter `book`. The stack already got `warm_up`, so that the redaction buffer of `observed`
/// on this thread can hold each value of a request like it.
fn routed_stack(class: Option<DataClass>, warm_up: HttpRequest) -> Telemetry<Responding> {
    let mut layer = Telemetry::server(reading_sink(2)).route_resolver(Arc::new(Route::resolver()));
    if let Some(class) = class {
        layer = layer.route_parameter_data_classes([("book", class)]);
    }
    let stack = layer.layer(common::responding(StatusCode::OK));
    drop(send(&stack, warm_up));
    stack
}

/// The server middleware with the request header `x-request-id` and the response header
/// `retry-after`, both with `class`, over a sink with two processors that read every field, in
/// front of [`Echoing`]. The stack already got `warm_up`, as for [`routed_stack`].
fn header_stack(class: DataClass, warm_up: HttpRequest) -> Telemetry<Echoing> {
    let stack = Telemetry::server(reading_sink(2))
        .request_header_data_classes([(REQUEST_ID, class.clone())])
        .response_header_data_classes([(RETRY_AFTER, class)])
        .layer(Echoing(StatusCode::OK));
    drop(send(&stack, warm_up));
    stack
}

/// A request for `/books/{book}` whose `book` is longer than 128 bytes. `Sensitive` writes such a
/// value into a `String` before its redactor reads it.
fn long_book() -> HttpRequest {
    Request::get(format!("http://example.com/books/1{}", "0".repeat(128)))
        .body(HttpBodyBuilder::new_fake().empty())
        .expect("the URI is valid")
}

/// A request for `/books/1` with the header `x-request-id`, and with `retry_after` as the header
/// `retry-after`. [`Echoing`] copies both headers to the response.
fn with_headers(retry_after: HeaderValue) -> HttpRequest {
    let mut request = common::get_to("http://example.com/books/1");
    request.headers_mut().insert(REQUEST_ID, HeaderValue::from_static("7"));
    request.headers_mut().insert(RETRY_AFTER, retry_after);
    request
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
#[bench::purge(Box::new(responding_stack(filtered_sink(|name| name == ACTIVE))), purge())]
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

#[metabench::benchmark(COMPLETE_STREAMED_BODY_WITH_SERVER_ADDRESS, GROUP, "complete_streamed_body_with_server_address")]
#[bench::get(
    Box::new(echoing_stack(reading_sink(2))),
    common::get_to("http://example.com/books/1").map(|_| common::scripted([common::data("chunk"), Step::End]))
)]
fn complete_streamed_body_with_server_address<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> S {
    read_body(stack.borrow(), request, 2);
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

#[metabench::benchmark(COMPLETE_WITH_ROUTE, GROUP, "complete_with_route")]
#[bench::get(
    Box::new(routed_stack(None, common::get_to("http://example.com/books"))),
    common::get_to("http://example.com/books")
)]
fn complete_with_route<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE_WITH_ROUTE_PARAMETERS, GROUP, "complete_with_route_parameters")]
#[bench::classified(
    Box::new(routed_stack(Some(PUBLIC), common::get_to("http://example.com/books/1"))),
    common::get_to("http://example.com/books/1")
)]
#[bench::classified_long(Box::new(routed_stack(Some(PUBLIC), long_book())), long_book())]
#[bench::erased(
    Box::new(routed_stack(Some(SECRET), common::get_to("http://example.com/books/1"))),
    common::get_to("http://example.com/books/1")
)]
fn complete_with_route_parameters<S: Borrow<Telemetry<Responding>>>(
    stack: S,
    request: HttpRequest,
) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE_WITH_HEADERS, GROUP, "complete_with_headers")]
#[bench::classified(
    Box::new(header_stack(PUBLIC, with_headers(HeaderValue::from_static("120")))),
    with_headers(HeaderValue::from_static("120"))
)]
#[bench::erased(
    Box::new(header_stack(SECRET, with_headers(HeaderValue::from_static("120")))),
    with_headers(HeaderValue::from_static("120"))
)]
#[bench::owned_value(
    Box::new(header_stack(SECRET, with_headers(HeaderValue::from_static("120")))),
    with_headers(HeaderValue::from_str("120").expect("120 is a valid header value"))
)]
fn complete_with_headers<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[expect(clippy::too_many_lines, reason = "one registration for each case of the group")]
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
    group.bench_function(BenchmarkId::new(ACTIVE_REQUESTS_ONLY.benchmark_name(), "purge"), |bencher| {
        bencher.iter_batched(purge, |request| active_requests_only(&stack, request).0, BatchSize::SmallInput);
    });
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
    let stack = echoing_stack(reading_sink(2));
    group.bench_function(
        BenchmarkId::new(COMPLETE_STREAMED_BODY_WITH_SERVER_ADDRESS.benchmark_name(), CASE),
        |bencher| {
            bencher.iter_batched(
                || common::get_to("http://example.com/books/1").map(|_| common::scripted([common::data("chunk"), Step::End])),
                |request| complete_streamed_body_with_server_address(&stack, request),
                BatchSize::SmallInput,
            );
        },
    );
    for (case, handler) in [("validation", common::FAILING), ("connection_refused", common::FAILING_CHAINED)] {
        group.bench_function(BenchmarkId::new(INNER_ERROR.benchmark_name(), case), |bencher| {
            bencher.iter_batched(common::get, |request| inner_error(&handler, request).0, BatchSize::SmallInput);
        });
        let stack = failing_stack(handler);
        group.bench_function(BenchmarkId::new(HANDLER_ERROR.benchmark_name(), case), |bencher| {
            bencher.iter_batched(common::get, |request| handler_error(&stack, request).0, BatchSize::SmallInput);
        });
    }
    let stack = routed_stack(None, common::get_to("http://example.com/books"));
    group.bench_function(BenchmarkId::new(COMPLETE_WITH_ROUTE.benchmark_name(), CASE), |bencher| {
        bencher.iter_batched(
            || common::get_to("http://example.com/books"),
            |request| complete_with_route(&stack, request).0,
            BatchSize::SmallInput,
        );
    });
    let book: fn() -> HttpRequest = || common::get_to("http://example.com/books/1");
    for (case, class, request) in [
        ("classified", Some(PUBLIC), book),
        ("classified_long", Some(PUBLIC), long_book),
        ("erased", Some(SECRET), book),
    ] {
        let stack = routed_stack(class, request());
        group.bench_function(BenchmarkId::new(COMPLETE_WITH_ROUTE_PARAMETERS.benchmark_name(), case), |bencher| {
            bencher.iter_batched(
                request,
                |request| complete_with_route_parameters(&stack, request).0,
                BatchSize::SmallInput,
            );
        });
    }
    let static_value: fn() -> HttpRequest = || with_headers(HeaderValue::from_static("120"));
    let owned_value: fn() -> HttpRequest = || with_headers(HeaderValue::from_str("120").expect("120 is a valid header value"));
    for (case, class, request) in [
        ("classified", PUBLIC, static_value),
        ("erased", SECRET, static_value),
        ("owned_value", SECRET, owned_value),
    ] {
        let stack = header_stack(class, static_value());
        group.bench_function(BenchmarkId::new(COMPLETE_WITH_HEADERS.benchmark_name(), case), |bencher| {
            bencher.iter_batched(request, |request| complete_with_headers(&stack, request).0, BatchSize::SmallInput);
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
        COMPLETE_STREAMED_BODY_WITH_SERVER_ADDRESS,
        INNER_ERROR,
        HANDLER_ERROR,
        COMPLETE_WITH_ROUTE,
        COMPLETE_WITH_ROUTE_PARAMETERS,
        COMPLETE_WITH_HEADERS,
    ],
);
