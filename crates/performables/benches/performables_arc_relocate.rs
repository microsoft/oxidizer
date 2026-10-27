// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures relocation of a handle onto an already materialized worker partition.
//! Timer and service handles revisit these partitions as runtime work moves.
//! Coordinates and partition initialization are outside the measured operation.
//! These are single-threaded, uncontended measurements, not NUMA-locality measurements.
//! Metabench also supplies allocation counts and Linux instruction counts.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::hint::black_box;
use std::ops::DerefMut;

use criterion::{BenchmarkId, Criterion};
use performables::arc::{Arc, PerNuma, PerThread};
use thread_aware::{Relocator, Thread, ThreadAware};

const WARM: &str = "performables_arc_relocate/warm";

struct WarmRelocation<T> {
    value: T,
    source: Thread,
    destination: Thread,
}

fn prepare<T: ThreadAware>(mut value: T, (source, destination): (Option<Thread>, Thread)) -> WarmRelocation<T> {
    let source = source.expect("between_threads and between_numa_nodes provide a source coordinate");
    value.relocate(Some(&source), &destination);
    value.relocate(Some(&destination), &source);
    WarmRelocation {
        value,
        source,
        destination,
    }
}

fn per_thread() -> WarmRelocation<Arc<u64, PerThread>> {
    prepare(
        Arc::<u64, PerThread>::new_with(|| 42),
        Relocator::between_threads().relocate(&mut ()),
    )
}

fn per_numa() -> WarmRelocation<Arc<u64, PerNuma>> {
    prepare(
        Arc::<u64, PerNuma>::new_with(|| 42),
        Relocator::between_numa_nodes().relocate(&mut ()),
    )
}

#[metabench::benchmark(RELOCATE_SAME, WARM, "same")]
#[bench::per_thread(Box::new(per_thread()))]
#[bench::per_numa(Box::new(per_numa()))]
fn relocate_same<T: ThreadAware, S: DerefMut<Target = WarmRelocation<T>>>(mut owned_input: S) -> ((), S) {
    {
        let input = &mut *owned_input;
        black_box(&mut input.value).relocate(Some(black_box(&input.source)), black_box(&input.source));
    }
    ((), owned_input)
}

#[metabench::benchmark(RELOCATE_ROUNDTRIP, WARM, "roundtrip")]
#[bench::per_thread(Box::new(per_thread()))]
#[bench::per_numa(Box::new(per_numa()))]
fn relocate_roundtrip<T: ThreadAware, S: DerefMut<Target = WarmRelocation<T>>>(mut owned_input: S) -> ((), S) {
    {
        let input = &mut *owned_input;
        black_box(&mut input.value).relocate(Some(black_box(&input.source)), black_box(&input.destination));
        black_box(&mut input.value).relocate(Some(black_box(&input.destination)), black_box(&input.source));
    }
    ((), owned_input)
}

fn measure<T: ThreadAware>(c: &mut Criterion, name: &str, mut input: WarmRelocation<T>) {
    let mut group = c.benchmark_group(WARM);
    group.bench_function(BenchmarkId::new(RELOCATE_SAME.benchmark_name(), name), |b| {
        b.iter(|| relocate_same(&mut input).0);
    });
    group.bench_function(BenchmarkId::new(RELOCATE_ROUNDTRIP.benchmark_name(), name), |b| {
        b.iter(|| relocate_roundtrip(&mut input).0);
    });
    group.finish();
}

fn benchmarks(c: &mut Criterion) {
    measure(c, "per_thread", per_thread());
    measure(c, "per_numa", per_numa());
}

metabench::main!(criterion = benchmarks, benchmarks = [RELOCATE_SAME, RELOCATE_ROUNDTRIP],);
