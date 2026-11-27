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
use std::future::ready;
use std::sync::Arc;
use std::task::Poll;

use common::{
    Echoing, Failing, NeverResponding, PUBLIC, REQUEST_ID, ReadAll, Responding, SECRET, abandon, filled, frames_32, in_memory,
    interested_sink, invalid_utf8, json, post, read_to_end, reading_sink, send, stalled, streamed_bytes, typed, with_headers,
};
use criterion::{BatchSize, BenchmarkId, Criterion};
use data_privacy::DataClass;
use http::header::RETRY_AFTER;
use http::{HeaderValue, StatusCode};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, HttpResponse, UriTemplateLabel};
use http_telemetry::{Client, Telemetry, TelemetryLayer};
use layered::{Layer, Service};
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

/// Polls the body of each request until it ends or is pending, as a transport does when it sends the request, and
/// then responds with 200 and an empty body. It does not allocate.
struct Sending(HttpBodyBuilder);

impl Service<HttpRequest> for Sending {
    type Out = http_extensions::Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let mut body = input.into_body();
        while let Poll::Ready(Some(_)) = common::poll_frame(&mut body) {}
        ready(Ok(HttpResponse::new(self.0.empty())))
    }
}

/// The client middleware over a sink with two processors that read every field, in front of [`Echoing`]. The stack
/// already got one request.
fn echoing_stack() -> Telemetry<Echoing> {
    let stack = Telemetry::client(reading_sink(2)).layer(Echoing(StatusCode::OK));
    drop(send(&stack, get()));
    stack
}

/// The client middleware that logs the bodies of `application/json` responses, over a sink with two processors that
/// read every field, in front of [`Echoing`]. The stack already got two such responses, one in memory and one streamed
/// in several frames.
fn response_logging_stack() -> Telemetry<Echoing> {
    let stack = Telemetry::client(reading_sink(2))
        .response_body_media_types(["application/json"])
        .expect("application/json is a media type")
        .layer(Echoing(StatusCode::OK));
    read_to_end(&stack, json(in_memory(b"{}")));
    read_to_end(&stack, json(streamed_bytes(65536)));
    stack
}

/// The client middleware over a sink with two processors that read every field, in front of [`Sending`]. The stack
/// already got one request.
fn sending_stack() -> Telemetry<Sending> {
    let stack = Telemetry::client(reading_sink(2)).layer(Sending(HttpBodyBuilder::new_fake()));
    drop(send(&stack, get()));
    stack
}

/// The client middleware that logs the bodies of `application/json` requests, over a sink with two processors that
/// read every field, in front of [`Sending`]. The stack already got one request.
fn request_logging_stack() -> Telemetry<Sending> {
    let stack = Telemetry::client(reading_sink(2))
        .request_body_media_types(["application/json"])
        .expect("application/json is a media type")
        .layer(Sending(HttpBodyBuilder::new_fake()));
    drop(send(&stack, post("application/json", streamed_bytes(1024))));
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

#[metabench::benchmark(FIELDS_COMPLETE_WITH_RESPONSE_BODY, FIELDS, "complete_with_response_body")]
#[bench::logging_off(Box::new(echoing_stack()), json(filled(1024)))]
#[bench::not_matching(Box::new(response_logging_stack()), typed("image/png", filled(1024)))]
#[bench::bytes_1024(Box::new(response_logging_stack()), json(filled(1024)))]
#[bench::frames_32(Box::new(response_logging_stack()), json(frames_32()))]
#[bench::invalid_utf8(Box::new(response_logging_stack()), json(in_memory(&invalid_utf8())))]
#[bench::bytes_65536(Box::new(response_logging_stack()), json(filled(65536)))]
#[bench::streamed_1024(Box::new(response_logging_stack()), json(streamed_bytes(1024)))]
#[bench::streamed_65536(Box::new(response_logging_stack()), json(streamed_bytes(65536)))]
fn fields_complete_with_response_body<S: Borrow<Telemetry<Echoing>>>(stack: S, request: HttpRequest) -> S {
    read_to_end(stack.borrow(), request);
    stack
}

#[metabench::benchmark(FIELDS_COMPLETE_WITH_REQUEST_BODY, FIELDS, "complete_with_request_body")]
#[bench::logging_off(Box::new(sending_stack()), post("application/json", streamed_bytes(1024)))]
#[bench::not_matching(Box::new(request_logging_stack()), post("image/png", streamed_bytes(1024)))]
#[bench::bytes_1024(Box::new(request_logging_stack()), post("application/json", filled(1024)))]
#[bench::invalid_utf8(Box::new(request_logging_stack()), post("application/json", in_memory(&invalid_utf8())))]
#[bench::streamed_1024(Box::new(request_logging_stack()), post("application/json", streamed_bytes(1024)))]
#[bench::streamed_65536(Box::new(request_logging_stack()), post("application/json", streamed_bytes(65536)))]
#[bench::stalled_1024_of_65536(Box::new(request_logging_stack()), post("application/json", stalled(1024, 65536)))]
fn fields_complete_with_request_body<S: Borrow<Telemetry<Sending>>>(
    stack: S,
    request: HttpRequest,
) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[metabench::benchmark(FIELDS_COMPLETE_READ_TWICE, FIELDS, "complete_read_twice")]
#[bench::redacted_path(
    Box::new(responding_stack(&Telemetry::client(Sink::composite([passthrough_sink(), passthrough_sink()])), get())),
    get()
)]
#[bench::method_original(Box::new(responding_stack(&Telemetry::client(reading_sink(2)), get())), common::purge())]
fn fields_complete_read_twice<S: Borrow<Telemetry<Responding>>>(stack: S, request: HttpRequest) -> (Result<HttpResponse, HttpError>, S) {
    (send(stack.borrow(), request), stack)
}

#[expect(clippy::too_many_lines, reason = "one registration for each case of the group")]
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
    let logging_off = echoing_stack();
    let logging_on = response_logging_stack();
    let json_1024: fn() -> HttpRequest = || json(filled(1024));
    for (case, stack, request) in [
        ("logging_off", &logging_off, json_1024),
        ("not_matching", &logging_on, || typed("image/png", filled(1024))),
        ("bytes_1024", &logging_on, json_1024),
        ("frames_32", &logging_on, || json(frames_32())),
        ("invalid_utf8", &logging_on, || json(in_memory(&invalid_utf8()))),
        ("bytes_65536", &logging_on, || json(filled(65536))),
        ("streamed_1024", &logging_on, || json(streamed_bytes(1024))),
        ("streamed_65536", &logging_on, || json(streamed_bytes(65536))),
    ] {
        group.bench_function(
            BenchmarkId::new(FIELDS_COMPLETE_WITH_RESPONSE_BODY.benchmark_name(), case),
            |bencher| {
                bencher.iter_batched(
                    request,
                    |request| fields_complete_with_response_body(stack, request),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    let logging_off = sending_stack();
    let logging_on = request_logging_stack();
    let streamed_1024: fn() -> HttpRequest = || post("application/json", streamed_bytes(1024));
    for (case, stack, request) in [
        ("logging_off", &logging_off, streamed_1024),
        ("not_matching", &logging_on, || post("image/png", streamed_bytes(1024))),
        ("bytes_1024", &logging_on, || post("application/json", filled(1024))),
        ("invalid_utf8", &logging_on, || post("application/json", in_memory(&invalid_utf8()))),
        ("streamed_1024", &logging_on, streamed_1024),
        ("streamed_65536", &logging_on, || post("application/json", streamed_bytes(65536))),
        ("stalled_1024_of_65536", &logging_on, || {
            post("application/json", stalled(1024, 65536))
        }),
    ] {
        group.bench_function(
            BenchmarkId::new(FIELDS_COMPLETE_WITH_REQUEST_BODY.benchmark_name(), case),
            |bencher| {
                bencher.iter_batched(
                    request,
                    |request| fields_complete_with_request_body(stack, request).0,
                    BatchSize::SmallInput,
                );
            },
        );
    }
    let stack = responding_stack(&Telemetry::client(Sink::composite([passthrough_sink(), passthrough_sink()])), get());
    group.bench_function(
        BenchmarkId::new(FIELDS_COMPLETE_READ_TWICE.benchmark_name(), "redacted_path"),
        |bencher| {
            bencher.iter_batched(get, |request| fields_complete_read_twice(&stack, request).0, BatchSize::SmallInput);
        },
    );
    let stack = responding_stack(&Telemetry::client(reading_sink(2)), get());
    group.bench_function(
        BenchmarkId::new(FIELDS_COMPLETE_READ_TWICE.benchmark_name(), "method_original"),
        |bencher| {
            bencher.iter_batched(
                common::purge,
                |request| fields_complete_read_twice(&stack, request).0,
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
        FIELDS_COMPLETE_WITH_RESPONSE_BODY,
        FIELDS_COMPLETE_WITH_REQUEST_BODY,
        FIELDS_COMPLETE_READ_TWICE,
    ],
);
