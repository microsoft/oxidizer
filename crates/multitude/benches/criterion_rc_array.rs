// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds an `Rc<[Rc<[u8]>]>` of `PROPERTIES` binary blobs two ways and
//! compares them: `std::rc::Rc` (global allocator) vs `multitude::Rc`
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(unused_results, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]
#![allow(dead_code, reason = "array properties are held only to keep the allocation alive")]

use std::hint::black_box;
use std::rc::Rc as StdRc;

use criterion::{Criterion, criterion_group, criterion_main};
use multitude::{Arena, Rc as ArenaRc};

// ---------------------------------------------------------------------------
// Array shape: `PROPERTIES` binary blobs of `PROPERTY_SIZE` bytes each.
// ---------------------------------------------------------------------------

const PROPERTIES: usize = 8;
const PROPERTY_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Global-allocator array
// ---------------------------------------------------------------------------

fn build_global(payload: &[u8]) -> StdRc<[StdRc<[u8]>]> {
    let mut properties = Vec::with_capacity(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(StdRc::<[u8]>::from(payload));
    }
    StdRc::from(properties)
}

fn build_global_from_slice(properties: &[StdRc<[u8]>]) -> StdRc<[StdRc<[u8]>]> {
    StdRc::from(properties)
}

// ---------------------------------------------------------------------------
// Arena-backed array
// ---------------------------------------------------------------------------

fn build_arena(arena: &Arena, payload: &[u8]) -> ArenaRc<[ArenaRc<[u8]>]> {
    let mut properties = arena.alloc_vec_with_capacity::<ArenaRc<[u8]>>(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(arena.alloc_slice_copy_rc(payload));
    }
    properties.try_into_rc_slice().unwrap()
}

fn build_arena_from_slice(arena: &Arena, properties: &[StdRc<[u8]>]) -> ArenaRc<[StdRc<[u8]>]> {
    arena.alloc_slice_clone_rc(properties)
}

fn global_properties(payload: &[u8]) -> Vec<StdRc<[u8]>> {
    (0..PROPERTIES).map(|_| StdRc::<[u8]>::from(payload)).collect()
}

// ---------------------------------------------------------------------------
// Criterion timing + per-iteration allocation tracking
// ---------------------------------------------------------------------------

fn bench_rc_array(c: &mut Criterion) {
    let payload = vec![0xABu8; PROPERTY_SIZE];

    let mut group = c.benchmark_group("rc_array");

    group.bench_function("global", |b| {
        b.iter(|| {
            black_box(build_global(black_box(&payload)));
        });
    });

    let arena = Arena::new();
    black_box(build_arena(&arena, &payload));

    group.bench_function("arena", |b| {
        b.iter(|| {
            black_box(build_arena(&arena, black_box(&payload)));
        });
    });

    let global_props = global_properties(&payload);
    group.bench_function("global_from_slice", |b| {
        b.iter(|| {
            black_box(build_global_from_slice(black_box(&global_props)));
        });
    });

    let work_arena = Arena::new();
    black_box(build_arena_from_slice(&work_arena, &global_props));

    group.bench_function("arena_from_slice", |b| {
        b.iter(|| {
            black_box(build_arena_from_slice(&work_arena, black_box(&global_props)));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_rc_array);
criterion_main!(benches);
