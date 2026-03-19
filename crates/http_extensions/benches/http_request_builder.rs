// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::missing_panics_doc,
    clippy::wildcard_imports,
    clippy::unwrap_used,
    missing_docs,
    reason = "improves readability in benchmarks"
)]

use alloc_tracker::{Allocator, Session};
use bytesbuf::mem::testing::TransparentMemory;
use criterion::{Criterion, criterion_group, criterion_main};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, Uri};
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpRequestBuilder};
use serde::Serialize;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const URI_STRING: &str = "https://example.com/api/v1/resource?query=value&page=1";

fn get_uri() -> Uri {
    URI_STRING.parse().expect("URI_STRING is a valid URI")
}

#[expect(clippy::too_many_lines, reason = "bench code, such is life")]
fn entry(c: &mut Criterion) {
    let session = Session::new();
    let mut group = c.benchmark_group("http_request_builder");
    let body_builder = HttpBodyBuilder::new_fake();

    // --- URI benchmarks ---

    let operation = session.operation("uri_from_string");
    group.bench_function("uri_from_string", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::GET)
                .uri(URI_STRING)
                .build()
                .unwrap();
        });
    });

    let uri: Uri = URI_STRING.parse().unwrap();
    let operation = session.operation("uri_pre_parsed");
    group.bench_function("uri_pre_parsed", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::GET)
                .uri(uri.clone())
                .build()
                .unwrap();
        });
    });

    // --- Body benchmarks ---

    let uri = get_uri();
    let operation = session.operation("empty_body");
    group.bench_function("empty_body", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::GET)
                .uri(uri.clone())
                .build()
                .unwrap();
        });
    });

    let uri = get_uri();
    let operation = session.operation("text_body");
    group.bench_function("text_body", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::POST)
                .uri(uri.clone())
                .text("Hello World!")
                .build()
                .unwrap();
        });
    });

    let uri = get_uri();
    let person = PersonOwned {
        name: "John".to_string(),
        surname: "Doe".to_string(),
        age: 30,
    };
    let operation = session.operation("json_body_owned");
    group.bench_function("json_body_owned", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::POST)
                .uri(uri.clone())
                .json(&person)
                .build()
                .unwrap();
        });
    });

    let uri = get_uri();
    let person = PersonBorrowed {
        name: "John",
        surname: "Doe",
        age: 30,
    };
    let operation = session.operation("json_body_borrowed");
    group.bench_function("json_body_borrowed", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::POST)
                .uri(uri.clone())
                .json(&person)
                .build()
                .unwrap();
        });
    });

    let uri = get_uri();
    let large_payload = LargePayload {
        items: (0..300)
            .map(|i| LargePayloadItem {
                id: i,
                name: format!("item-name-{i:04}"),
                description: format!("This is a longer description for item number {i:04}"),
                value: f64::from(i) * 1.5,
            })
            .collect(),
    };
    // Use TransparentMemory instead of GlobalPool so that every reserve() call from the
    // serde_json writer results in a real heap allocation. This makes alloc_tracker report
    // the true number of memory reservations, which GlobalPool would otherwise absorb.
    let transparent_body_builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new());
    let operation = session.operation("json_body_large_transparent");
    group.bench_function("json_body_large_transparent", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&transparent_body_builder)
                .method(Method::POST)
                .uri(uri.clone())
                .json(&large_payload)
                .build()
                .unwrap();
        });
    });

    // --- Header benchmarks ---

    let uri = get_uri();
    let operation = session.operation("no_header");
    group.bench_function("no_header", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::GET)
                .uri(uri.clone())
                .build()
                .unwrap();
        });
    });

    let uri = get_uri();
    let operation = session.operation("single_header");
    group.bench_function("single_header", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::GET)
                .uri(uri.clone())
                .build()
                .unwrap();
        });
    });

    let uri = get_uri();
    let operation = session.operation("two_headers");
    group.bench_function("two_headers", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _request: HttpRequest = HttpRequestBuilder::new(&body_builder)
                .method(Method::GET)
                .uri(uri.clone())
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .build()
                .unwrap();
        });
    });

    group.finish();
    session.print_to_stdout();
}

criterion_group!(benches, entry);
criterion_main!(benches);

#[derive(Serialize, Debug)]
struct PersonOwned {
    name: String,
    surname: String,
    age: u32,
}

#[derive(Serialize, Debug)]
struct PersonBorrowed<'a> {
    name: &'a str,
    surname: &'a str,
    age: u32,
}

#[derive(Serialize, Debug)]
struct LargePayload {
    items: Vec<LargePayloadItem>,
}

#[derive(Serialize, Debug)]
struct LargePayloadItem {
    id: u32,
    name: String,
    description: String,
    value: f64,
}
