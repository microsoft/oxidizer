// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds an `Arc<[Arc<[u8]>]>` of `PROPERTIES` binary blobs two ways and
//! compares them: `std::sync::Arc` (global allocator) vs `multitude::Arc`
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(unused_results, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]
#![allow(dead_code, reason = "array properties are held only to keep the allocation alive")]

use std::hint::black_box;
use std::sync::Arc as StdArc;

use criterion::{Criterion, criterion_group, criterion_main};
use multitude::{Arc as ArenaArc, Arena};

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

    let mut group = c.benchmark_group("arc_array");

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

criterion_group!(benches, bench_arc_array);
criterion_main!(benches);
