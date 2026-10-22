// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures relocation of a handle onto an already materialized worker partition.
//! Timer and service handles revisit these partitions as runtime work moves.
//! Coordinates and partition initialization are outside the measured operation.
//! These are single-threaded, uncontended measurements, not NUMA-locality measurements.

#![expect(clippy::missing_docs_in_private_items, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use performables::arc::{Arc, PerNuma, PerThread};
use thread_aware::{Relocator, Thread, ThreadAware};

fn measure<T: ThreadAware>(c: &mut Criterion, name: &str, mut value: T, source: &Thread, destination: &Thread) {
    value.relocate(Some(source), destination);
    value.relocate(Some(destination), source);

    let mut group = c.benchmark_group("performables_arc_relocate/warm");
    group.bench_function(format!("{name}_same"), |b| {
        b.iter(|| {
            black_box(&mut value).relocate(Some(black_box(source)), black_box(source));
        });
    });
    group.bench_function(format!("{name}_roundtrip"), |b| {
        b.iter(|| {
            black_box(&mut value).relocate(Some(black_box(source)), black_box(destination));
            black_box(&mut value).relocate(Some(black_box(destination)), black_box(source));
        });
    });
    group.finish();
}

fn benchmarks(c: &mut Criterion) {
    let (source, destination) = Relocator::between_threads().relocate(&mut ());
    let source = source.expect("between_threads provides a source coordinate");
    measure(c, "per_thread", Arc::<u64, PerThread>::new_with(|| 42), &source, &destination);

    let (source, destination) = Relocator::between_numa_nodes().relocate(&mut ());
    let source = source.expect("between_numa_nodes provides a source coordinate");
    measure(c, "per_numa", Arc::<u64, PerNuma>::new_with(|| 42), &source, &destination);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
