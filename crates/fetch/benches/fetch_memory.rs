// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Steady-state reservations and body operations through long-lived providers.

#![allow(missing_docs, clippy::unwrap_used, reason = "benchmark code")]

use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample;
use bytesbuf::BytesView;
use bytesbuf::mem::{HasMemory, Memory};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main};
use fetch::{HttpBodyBuilder, HttpClient};
use futures::executor::block_on;
use http::StatusCode;
use http_extensions::HttpBodyOptions;
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Compares public allocation paths without client construction or network I/O.
fn memory(c: &mut Criterion) {
    let session = Session::new();
    let clock = Clock::new_frozen();
    let client = HttpClient::builder_fake(StatusCode::OK, &clock).minimal_pipeline().build();
    let retained = client.memory();
    let builder = HttpBodyBuilder::new_fake();
    let mut group = c.benchmark_group("fetch_memory/steady_state");

    // A short control message and a larger body expose fixed overhead and copying costs.
    for size in [64, 64 * 1024] {
        let payload = "x".repeat(size);
        // Split a response into equal frames to exercise buffered streaming assembly.
        let frame = BytesView::copied_from_slice(&payload.as_bytes()[..size / 2], &builder);
        operation(&mut group, &session, "client_reserve", size, || {
            black_box(&client).reserve(black_box(size))
        });
        operation(&mut group, &session, "retained_reserve", size, || {
            black_box(&retained).reserve(black_box(size))
        });
        operation(&mut group, &session, "body_reserve", size, || {
            black_box(&builder).reserve(black_box(size))
        });
        operation(&mut group, &session, "body_slice", size, || {
            black_box(&builder).slice(black_box(payload.as_bytes()))
        });
        operation(&mut group, &session, "body_json", size, || {
            black_box(&builder).json(black_box(&payload)).unwrap()
        });
        operation(&mut group, &session, "stream_read", size, || {
            let frames = [Ok(frame.clone()), Ok(frame.clone())];
            let body = black_box(&builder).stream(futures::stream::iter(frames), &HttpBodyOptions::default());
            block_on(body.into_buffered()).unwrap()
        });
    }
    group.finish();
}

/// Gives each operation its own typed loop, with no case selection inside the timed region.
fn operation<R>(group: &mut BenchmarkGroup<'_, WallTime>, session: &Session, name: &str, size: usize, mut run: impl FnMut() -> R) {
    let allocations = session.operation(format!("{name}/{size}"));
    group.bench_with_input(BenchmarkId::new(name, size), &size, |b, _| {
        b.iter_custom(|iters| {
            let _measurement = allocations.measure_thread().iterations(iters);
            time_sample(iters, &mut run)
        });
    });
}

criterion_group!(benches, memory);
criterion_main!(benches);
