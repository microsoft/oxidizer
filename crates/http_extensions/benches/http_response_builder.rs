// Copyright (c) Microsoft Corporation.

#![allow(
    clippy::missing_panics_doc,
    clippy::wildcard_imports,
    clippy::unwrap_used,
    missing_docs,
    reason = "improves readability in benchmarks"
)]

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use http_extensions::{HttpBodyBuilder, HttpResponseBuilder};
use serde::{Deserialize, Serialize};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn bodies_benchmarks(c: &mut Criterion) {
    let session = Session::new();
    let mut group = c.benchmark_group("http_response_builder_bodies");
    let body_builder = HttpBodyBuilder::new_fake();

    let operation = session.operation("empty_body");
    group.bench_function("empty_body", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _response = HttpResponseBuilder::new(&body_builder).build().unwrap();
        });
    });

    let operation = session.operation("text_body");
    group.bench_function("text_body", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _response = HttpResponseBuilder::new(&body_builder)
                .text("Hello, world!")
                .build()
                .unwrap();
        });
    });

    let person = PersonOwned {
        name: "John".to_string(),
        surname: "Doe".to_string(),
        age: 30,
    };
    let operation = session.operation("json_body_owned");
    group.bench_function("json_body_owned", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _response = HttpResponseBuilder::new(&body_builder)
                .json(&person)
                .build()
                .unwrap();
        });
    });

    let person = PersonBorrowed {
        name: "John",
        surname: "Doe",
        age: 30,
    };
    let operation = session.operation("json_body_borrowed");
    group.bench_function("json_body_borrowed", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            let _response = HttpResponseBuilder::new(&body_builder)
                .json(&person)
                .build()
                .unwrap();
        });
    });

    group.finish();
    session.print_to_stdout();
}

criterion_group!(benches, bodies_benchmarks);
criterion_main!(benches);

#[derive(Serialize, Deserialize, Debug)]
struct PersonOwned {
    name: String,
    surname: String,
    age: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct PersonBorrowed<'a> {
    name: &'a str,
    surname: &'a str,
    age: u32,
}
