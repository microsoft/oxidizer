// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Steady-state allocation and free costs with event recording disabled or enabled.

#![allow(missing_docs, reason = "benchmark code")]

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};

// SAFETY: The benchmark returns every block to this linked snmalloc instance.
const ALLOCATOR: rallocator::Rallocator = unsafe { rallocator::Rallocator::new(rallocator::backend::Snmalloc) };

// Keep the same measured call boundary in both recording modes.
#[inline(never)]
fn allocation_pair(allocator: &impl GlobalAlloc) {
    // A small fixed class isolates ordinary per-owner allocation and free work.
    let layout = Layout::new::<[u8; 64]>();
    // SAFETY: The allocator receives a valid nonzero layout.
    let address = unsafe { allocator.alloc(layout) };
    if address.is_null() {
        handle_alloc_error(layout);
    }
    black_box(address);
    // SAFETY: The allocation remains live and uses its original allocator and layout.
    unsafe { allocator.dealloc(address, layout) };
}

fn benchmark(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("rallocator_telemetry/allocation_pair_64");
    for (name, enabled) in [("events_off", false), ("events_on", true)] {
        seismograph::recorder(Configuration {
            allocations: if enabled { Policy::all(false) } else { Policy::default() },
            // The minimum ring reaches steady-state overwrites during warmup.
            event_capacity_per_thread: EventBufferCapacity::new(64),
            ..Default::default()
        });
        group.bench_function(name, |bencher| bencher.iter(|| allocation_pair(&ALLOCATOR)));
    }
    group.finish();
    seismograph::recorder(Configuration::default());

    let mut group = criterion.benchmark_group("rallocator_telemetry/dispatch_pair_64");
    // SAFETY: allocation_pair returns each block to the handle that created it.
    let mimalloc = unsafe { rallocator::Rallocator::new(rallocator::backend::Mimalloc) };
    // SAFETY: The dynamic handle also uses one backend for the whole process.
    let dynamic = unsafe { rallocator::Rallocator::new(rallocator::backend::Dynamic) };
    group.bench_function("snmalloc", |b| b.iter(|| allocation_pair(&ALLOCATOR)));
    group.bench_function("mimalloc", |b| b.iter(|| allocation_pair(&mimalloc)));
    group.bench_function("system", |b| b.iter(|| allocation_pair(&System)));
    let name = match std::env::var("RALLOCATOR_BACKEND").as_deref() {
        Ok("mimalloc") => "dynamic_mimalloc",
        Ok("system") => "dynamic_system",
        _ => "dynamic_snmalloc",
    };
    group.bench_function(name, |b| b.iter(|| allocation_pair(&dynamic)));
    group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
