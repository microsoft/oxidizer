// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The overhead that the client middleware adds to one request.
//!
//! Each stack gets one request before the measurement. At the first request of a sink on a thread,
//! `observed` allocates storage for that thread. The allocation count of `metabench` covers only
//! the first call of a case, so without that request it also counts this storage.

#[path = "../tests/common/mod.rs"]
mod common;

use std::borrow::Borrow;
use std::sync::Arc;

use common::{
    Echoing, Failing, NeverResponding, PUBLIC, REQUEST_ID, ReadAll, Responding, SECRET, abandon, interested_sink, reading_sink, send,
    with_headers,
};
use criterion::{BatchSize, BenchmarkId, Criterion};
use data_privacy::DataClass;
use http::header::RETRY_AFTER;
use http::{HeaderValue, StatusCode};
use http_extensions::{HttpError, HttpRequest, HttpRequestBuilder, HttpResponse, UriTemplateLabel};
use http_telemetry::{Client, Telemetry, TelemetryLayer};
use layered::Layer;
use observed::metadata::FieldDescriptor;
use observed::processing::EventProcessor;
use observed::{Sink, SinkId};
use tick::SimpleClock;

const ATTEMPT: &str = "http_telemetry_client/attempt";
const TOTAL: &str = "http_telemetry_client/total";
const FIELDS: &str = "http_telemetry_client/fields";

/// A `GET` request over HTTP/1.1 to `/books/1`. The URI keeps the static text, so a clone of its
/// path does not allocate.
fn get() -> HttpRequest {
    common::get_to("/books/1")
}

/// A `GET` request to `https://example.com/books/1`, built by `HttpRequestBuilder` from the text of
/// the URL, so its path has no template.
fn plain_path() -> HttpRequest {
    HttpRequestBuilder::new_fake()
        .get("https://example.com/books/1")
        .build()
        .expect("the URL is valid")
}

/// A `GET` request to `ftp://example.com/books/1`, without a template.
fn custom_scheme() -> HttpRequest {
    common::get_to("ftp://example.com/books/1")
}

/// The request of [`common::get_book`], with a `UriTemplateLabel` that owns its text.
fn owned_label() -> HttpRequest {
    let mut request = common::get_book();
    request.extensions_mut().insert(UriTemplateLabel::new(String::from("book_by_id")));
    request
}

/// A sink with a log processor, which reads the log fields of every event, and a metric processor,
/// which reads its metric fields. Both print every value unchanged.
fn passthrough_sink() -> Sink {
    let readers: [fn(&FieldDescriptor) -> bool; 2] = [|descriptor| descriptor.log().is_some(), |descriptor| descriptor.metric().is_some()];
    Sink::builder(SinkId::new("passthrough"))
        .clock(SimpleClock::new_frozen())
        .processors(
            readers
                .into_iter()
                .map(|reads| {
                    Arc::new(ReadAll {
                        engine: observed_testing::passthrough_redaction_engine(),
                        interested: |_| true,
                        reads,
                    }) as Arc<dyn EventProcessor>
                })
                .collect(),
        )
        .build()
}

/// The middleware of `layer`, in front of [`Responding`]. The stack already got `warm_up`, so that
/// the redaction buffer of `observed` on this thread can hold each value of a request like it.
fn responding_stack(layer: &TelemetryLayer<Client>, warm_up: HttpRequest) -> Telemetry<Responding> {
    let stack = layer.layer(common::responding(StatusCode::OK));
    drop(send(&stack, warm_up));
    stack
}

/// The middleware of `layer`, in front of [`NeverResponding`]. The stack already got one request.
fn never_responding_stack(layer: &TelemetryLayer<Client>) -> Telemetry<NeverResponding> {
    let stack = layer.layer(NeverResponding);
    abandon(&stack, get());
    stack
}

/// The middleware of `layer`, in front of `handler`. The stack already got `warm_up`.
fn failing_stack(layer: &TelemetryLayer<Client>, handler: Failing, warm_up: HttpRequest) -> Telemetry<Failing> {
    let stack = layer.layer(handler);
    drop(send(&stack, warm_up));
    stack
}

/// A client layer with the three fields that `fetch` adds to tell which client sent a request.
fn client_attributes_layer() -> TelemetryLayer<Client> {
    Telemetry::client(passthrough_sink())
        .dimension("http.client.name", "books")
        .dimension("fetch.runtime", "tokio")
        .dimension("fetch.transport", "hyper")
}

/// The client middleware with the request header `x-request-id` and the response header `retry-after`, both with
/// `class`, over a sink with two processors that read every field, in front of [`Echoing`]. The stack already got
/// `warm_up`, as for [`responding_stack`].
fn header_stack(class: DataClass, warm_up: HttpRequest) -> Telemetry<Echoing> {
    let stack = Telemetry::client(reading_sink(2))
        .request_header_data_classes([(REQUEST_ID, class.clone())])
        .response_header_data_classes([(RETRY_AFTER, class)])
        .layer(Echoing(StatusCode::OK));
    drop(send(&stack, warm_up));
    stack
}

#[metabench::benchmark(INNER_ONLY, ATTEMPT, "inner_only")]
#[bench::get(Box::new(common::responding(StatusCode::OK)), get())]
fn inner_only<S: Borrow<Responding>>(handler: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(handler.borrow(), request), handler)
}

#[metabench::benchmark(UNINTERESTED_SINK, ATTEMPT, "uninterested_sink")]
#[bench::get(Box::new(responding_stack(&Telemetry::client(Sink::noop()), get())), get())]
fn uninterested_sink<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE, ATTEMPT, "complete")]
#[bench::get(Box::new(responding_stack(&Telemetry::client(reading_sink(1)), get())), get())]
fn complete<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(COMPLETE_OTHER_METHOD, ATTEMPT, "complete_other_method")]
#[bench::purge(Box::new(responding_stack(&Telemetry::client(reading_sink(1)), get())), common::purge())]
#[bench::long_extension_method(
    Box::new(responding_stack(&Telemetry::client(interested_sink()), get())),
    common::long_extension_method()
)]
fn complete_other_method<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(ABANDONED, ATTEMPT, "abandoned")]
#[bench::get(Box::new(never_responding_stack(&Telemetry::client(reading_sink(1)))), get())]
fn abandoned<S: Borrow<Telemetry<NeverResponding>>>(stack: S, request: HttpRequest) -> S {
    abandon(stack.borrow(), request);
    stack
}

#[metabench::benchmark(INNER_ERROR, ATTEMPT, "inner_error")]
#[bench::validation(Box::new(common::FAILING), get())]
#[bench::connection_refused(Box::new(common::FAILING_CHAINED), get())]
fn inner_error<S: Borrow<Failing>>(handler: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(handler.borrow(), request), handler)
}

#[metabench::benchmark(ERROR, ATTEMPT, "error")]
#[bench::validation(Box::new(failing_stack(&Telemetry::client(reading_sink(2)), common::FAILING, get())), get())]
#[bench::connection_refused(
    Box::new(failing_stack(&Telemetry::client(reading_sink(2)), common::FAILING_CHAINED, get())),
    get()
)]
fn error<S: Borrow<Telemetry<Failing>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(TOTAL_COMPLETE, TOTAL, "complete")]
#[bench::get(
    Box::new(responding_stack(&Telemetry::client(reading_sink(1)).report_total_duration(true), get())),
    get()
)]
fn total_complete<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(FIELDS_COMPLETE, FIELDS, "complete")]
#[bench::templated_path(
    Box::new(responding_stack(&Telemetry::client(passthrough_sink()), common::get_book())),
    common::get_book()
)]
#[bench::plain_path(Box::new(responding_stack(&Telemetry::client(passthrough_sink()), plain_path())), plain_path())]
#[bench::owned_label(Box::new(responding_stack(&Telemetry::client(passthrough_sink()), owned_label())), owned_label())]
#[bench::custom_scheme(
    Box::new(responding_stack(&Telemetry::client(passthrough_sink()), custom_scheme())),
    custom_scheme()
)]
#[bench::client_attributes(Box::new(responding_stack(&client_attributes_layer(), common::get_book())), common::get_book())]
fn fields_complete<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(FIELDS_COMPLETE_WITH_HEADERS, FIELDS, "complete_with_headers")]
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
fn fields_complete_with_headers<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(FIELDS_HANDLER_ERROR, FIELDS, "handler_error")]
#[bench::templated_path(
    Box::new(failing_stack(&Telemetry::client(passthrough_sink()), common::FAILING, common::get_book())),
    common::get_book()
)]
fn fields_handler_error<S: Borrow<Telemetry<Failing>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ATTEMPT);
    let handler = common::responding(StatusCode::OK);
    group.bench_function(BenchmarkId::new(INNER_ONLY.benchmark_name(), "get"), |bencher| {
        bencher.iter_batched(get, |request| inner_only(&handler, request).0, BatchSize::SmallInput);
    });
    let stack = responding_stack(&Telemetry::client(Sink::noop()), get());
    group.bench_function(BenchmarkId::new(UNINTERESTED_SINK.benchmark_name(), "get"), |bencher| {
        bencher.iter_batched(get, |request| uninterested_sink(&stack, request).0, BatchSize::SmallInput);
    });
    let stack = responding_stack(&Telemetry::client(reading_sink(1)), get());
    group.bench_function(BenchmarkId::new(COMPLETE.benchmark_name(), "get"), |bencher| {
        bencher.iter_batched(get, |request| complete(&stack, request).0, BatchSize::SmallInput);
    });
    group.bench_function(BenchmarkId::new(COMPLETE_OTHER_METHOD.benchmark_name(), "purge"), |bencher| {
        bencher.iter_batched(
            common::purge,
            |request| complete_other_method(&stack, request).0,
            BatchSize::SmallInput,
        );
    });
    let stack = responding_stack(&Telemetry::client(interested_sink()), get());
    group.bench_function(
        BenchmarkId::new(COMPLETE_OTHER_METHOD.benchmark_name(), "long_extension_method"),
        |bencher| {
            bencher.iter_batched(
                common::long_extension_method,
                |request| complete_other_method(&stack, request).0,
                BatchSize::SmallInput,
            );
        },
    );
    let stack = never_responding_stack(&Telemetry::client(reading_sink(1)));
    group.bench_function(BenchmarkId::new(ABANDONED.benchmark_name(), "get"), |bencher| {
        bencher.iter_batched(get, |request| abandoned(&stack, request), BatchSize::SmallInput);
    });
    for (case, handler) in [("validation", common::FAILING), ("connection_refused", common::FAILING_CHAINED)] {
        group.bench_function(BenchmarkId::new(INNER_ERROR.benchmark_name(), case), |bencher| {
            bencher.iter_batched(get, |request| inner_error(&handler, request).0, BatchSize::SmallInput);
        });
        let stack = failing_stack(&Telemetry::client(reading_sink(2)), handler, get());
        group.bench_function(BenchmarkId::new(ERROR.benchmark_name(), case), |bencher| {
            bencher.iter_batched(get, |request| error(&stack, request).0, BatchSize::SmallInput);
        });
    }
    group.finish();

    let mut group = criterion.benchmark_group(TOTAL);
    let stack = responding_stack(&Telemetry::client(reading_sink(1)).report_total_duration(true), get());
    group.bench_function(BenchmarkId::new(TOTAL_COMPLETE.benchmark_name(), "get"), |bencher| {
        bencher.iter_batched(get, |request| total_complete(&stack, request).0, BatchSize::SmallInput);
    });
    group.finish();

    let mut group = criterion.benchmark_group(FIELDS);
    let mut complete_case = |case, layer: &TelemetryLayer<Client>, request: fn() -> HttpRequest| {
        let stack = responding_stack(layer, request());
        group.bench_function(BenchmarkId::new(FIELDS_COMPLETE.benchmark_name(), case), |bencher| {
            bencher.iter_batched(request, |request| fields_complete(&stack, request).0, BatchSize::SmallInput);
        });
    };
    complete_case("templated_path", &Telemetry::client(passthrough_sink()), common::get_book);
    complete_case("plain_path", &Telemetry::client(passthrough_sink()), plain_path);
    complete_case("owned_label", &Telemetry::client(passthrough_sink()), owned_label);
    complete_case("custom_scheme", &Telemetry::client(passthrough_sink()), custom_scheme);
    complete_case("client_attributes", &client_attributes_layer(), common::get_book);
    let static_value: fn() -> HttpRequest = || with_headers(HeaderValue::from_static("120"));
    let owned_value: fn() -> HttpRequest = || with_headers(HeaderValue::from_str("120").expect("120 is a valid header value"));
    for (case, class, request) in [
        ("classified", PUBLIC, static_value),
        ("erased", SECRET, static_value),
        ("owned_value", SECRET, owned_value),
    ] {
        let stack = header_stack(class, static_value());
        group.bench_function(BenchmarkId::new(FIELDS_COMPLETE_WITH_HEADERS.benchmark_name(), case), |bencher| {
            bencher.iter_batched(
                request,
                |request| fields_complete_with_headers(&stack, request).0,
                BatchSize::SmallInput,
            );
        });
    }
    let stack = failing_stack(&Telemetry::client(passthrough_sink()), common::FAILING, common::get_book());
    group.bench_function(
        BenchmarkId::new(FIELDS_HANDLER_ERROR.benchmark_name(), "templated_path"),
        |bencher| {
            bencher.iter_batched(
                common::get_book,
                |request| fields_handler_error(&stack, request).0,
                BatchSize::SmallInput,
            );
        },
    );
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        INNER_ONLY,
        UNINTERESTED_SINK,
        COMPLETE,
        COMPLETE_OTHER_METHOD,
        ABANDONED,
        INNER_ERROR,
        ERROR,
        TOTAL_COMPLETE,
        FIELDS_COMPLETE,
        FIELDS_COMPLETE_WITH_HEADERS,
        FIELDS_HANDLER_ERROR,
    ],
);
