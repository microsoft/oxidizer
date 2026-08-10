// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation benchmarks for serde timestamp deserialization.
//!
//! Paired with `tick_serde_deserialize_cg.rs`, which measures the same
//! operations under Callgrind.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]

use std::alloc::System;
use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample;
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use serde::de::DeserializeOwned;
use tick::fmt::{EcmaScript, Iso8601, Rfc2822, UnixSeconds};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const ISO_8601: &str = r#""2024-08-06T21:30:00Z""#;
const RFC_2822: &str = r#""Tue, 06 Aug 2024 21:30:00 GMT""#;
const UNIX_SECONDS: &str = "1722979800";
const ECMASCRIPT: &str = r#""2024-08-06T21:30:00.123Z""#;

fn bench_format<T>(group: &mut BenchmarkGroup<'_, WallTime>, session: &Session, name: &str, input: &'static str)
where
    T: DeserializeOwned,
{
    let operation = session.operation(name);
    group.bench_function(name, |bencher| {
        bencher.iter_custom(|iterations| {
            let _measurement = operation.measure_thread().iterations(iterations);
            time_sample(iterations, || {
                serde_json::from_str::<T>(black_box(input)).expect("benchmark input is valid")
            })
        });
    });
}

fn deserialize(c: &mut Criterion) {
    // Initialize RFC 2822's lower-bound LazyLock before measurements begin.
    _ = serde_json::from_str::<Rfc2822>(RFC_2822).expect("RFC 2822 benchmark input is valid");

    let session = Session::new();
    let mut group = c.benchmark_group("tick_serde_deserialize/formats");

    bench_format::<Iso8601>(&mut group, &session, "iso_8601", ISO_8601);
    bench_format::<Rfc2822>(&mut group, &session, "rfc_2822", RFC_2822);
    bench_format::<UnixSeconds>(&mut group, &session, "unix_seconds", UNIX_SECONDS);
    bench_format::<EcmaScript>(&mut group, &session, "ecmascript", ECMASCRIPT);

    group.finish();
}

criterion_group!(benches, deserialize);
criterion_main!(benches);
