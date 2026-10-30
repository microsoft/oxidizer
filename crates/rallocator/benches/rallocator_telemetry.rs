// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Steady-state allocation and free costs with event recording disabled or enabled.

#![allow(missing_docs, reason = "benchmark code")]

use std::alloc::{GlobalAlloc, Layout, handle_alloc_error};
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};

// Keep the same measured call boundary in both recording modes.
#[inline(never)]
fn allocation_pair() {
    let layout = Layout::new::<[u8; 64]>();
    // SAFETY: The allocator receives a valid nonzero layout.
    let address = unsafe { rallocator::Rallocator.alloc(layout) };
    if address.is_null() {
        handle_alloc_error(layout);
    }
    black_box(address);
    // SAFETY: The allocation remains live and uses its original allocator and layout.
    unsafe { rallocator::Rallocator.dealloc(address, layout) };
}

fn benchmark(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("rallocator_telemetry/allocation_pair_64");
    for (name, enabled) in [("events_off", false), ("events_on", true)] {
        seismograph::recorder(Configuration {
            allocations: if enabled { Policy::all(false) } else { Policy::default() },
            event_capacity_per_thread: EventBufferCapacity::new(64),
            ..Default::default()
        });
        group.bench_function(name, |bencher| bencher.iter(allocation_pair));
    }
    group.finish();
    seismograph::recorder(Configuration::default());
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
