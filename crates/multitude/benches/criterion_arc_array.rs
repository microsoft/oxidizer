// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds an `Arc<[Arc<[u8]>]>` of `PROPERTIES` binary blobs two ways and
//! compares them: `std::sync::Arc` (global allocator) vs `multitude::Arc`
//! (arena, `reset` and reused between iterations).
//!
//! Each is benchmarked with two strategies:
//!
//! - `*` — push freshly allocated properties through a growable vec, then
//!   freeze it into the `Arc`.
//! - `*_from_slice` — build directly from a pre-created slice of properties,
//!   with no intermediate vec.
//!
//! Time is measured with **criterion**; per-iteration allocation volume (bytes
//! + count) with the [`alloc_tracker`] crate, printed after the timings.
//!
//! Run with: `cargo bench --bench criterion_arc_array`
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(unused_results, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]
#![allow(dead_code, reason = "array properties are held only to keep the allocation alive")]

use std::hint::black_box;
use std::sync::Arc as StdArc;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use multitude::{Arc as ArenaArc, Arena};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// ---------------------------------------------------------------------------
// Array shape: `PROPERTIES` binary blobs of `PROPERTY_SIZE` bytes each.
// ---------------------------------------------------------------------------

const PROPERTIES: usize = 8;
const PROPERTY_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Global-allocator array
// ---------------------------------------------------------------------------

fn build_global(payload: &[u8]) -> StdArc<[StdArc<[u8]>]> {
    let mut properties = Vec::with_capacity(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(StdArc::<[u8]>::from(payload));
    }
    StdArc::from(properties)
}

fn build_global_from_slice(properties: &[StdArc<[u8]>]) -> StdArc<[StdArc<[u8]>]> {
    StdArc::from(properties)
}

// ---------------------------------------------------------------------------
// Arena-backed array
// ---------------------------------------------------------------------------

fn build_arena(arena: &Arena, payload: &[u8]) -> ArenaArc<[ArenaArc<[u8]>]> {
    let mut properties = arena.alloc_vec_with_capacity::<ArenaArc<[u8]>>(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(arena.alloc_slice_copy_arc(payload));
    }
    properties.try_into_arc().unwrap()
}

fn build_arena_from_slice(arena: &Arena, properties: &[StdArc<[u8]>]) -> ArenaArc<[StdArc<[u8]>]> {
    arena.alloc_slice_clone_arc(properties)
}

fn global_properties(payload: &[u8]) -> Vec<StdArc<[u8]>> {
    (0..PROPERTIES).map(|_| StdArc::<[u8]>::from(payload)).collect()
}

// ---------------------------------------------------------------------------
// Criterion timing + per-iteration allocation tracking
// ---------------------------------------------------------------------------

fn bench_arc_array(c: &mut Criterion) {
    let payload = vec![0xABu8; PROPERTY_SIZE];

    // Each `iter` takes a thread span so the per-op allocation mean is printed
    // alongside the timings.
    let session = Session::new();

    let mut group = c.benchmark_group("arc_array");

    let global_op = session.operation("global");
    group.bench_function("global", |b| {
        b.iter(|| {
            let _span = global_op.measure_thread();
            black_box(build_global(black_box(&payload)));
        });
    });

    // Warm the arena, then reset and reuse it each iteration.
    let mut arena = Arena::new();
    black_box(build_arena(&arena, &payload));
    arena.reset();

    let arena_op = session.operation("arena");
    group.bench_function("arena", |b| {
        b.iter(|| {
            let _span = arena_op.measure_thread();
            black_box(build_arena(&arena, black_box(&payload)));
            arena.reset();
        });
    });

    // The pre-created global properties feed both `*_from_slice` variants.
    let global_props = global_properties(&payload);
    let global_slice_op = session.operation("global_from_slice");
    group.bench_function("global_from_slice", |b| {
        b.iter(|| {
            let _span = global_slice_op.measure_thread();
            black_box(build_global_from_slice(black_box(&global_props)));
        });
    });

    let mut work_arena = Arena::new();
    black_box(build_arena_from_slice(&work_arena, &global_props));
    work_arena.reset();

    let arena_slice_op = session.operation("arena_from_slice");
    group.bench_function("arena_from_slice", |b| {
        b.iter(|| {
            let _span = arena_slice_op.measure_thread();
            black_box(build_arena_from_slice(&work_arena, black_box(&global_props)));
            work_arena.reset();
        });
    });

    group.finish();

    session.print_to_stdout();
}

criterion_group!(benches, bench_arc_array);
criterion_main!(benches);
