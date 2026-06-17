// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise `Arc<[Arc<[u8]>]>` build benchmarks for multitude.
//!
//! Mirrors `benches/criterion_arc_array.rs` 1:1: each gungraun function
//! `<variant>` corresponds to a criterion benchmark `arc_array/<variant>`.
//! Builds an `Arc<[Arc<[u8]>]>` of `PROPERTIES` binary blobs two ways and
//! compares them: `std::sync::Arc` (global allocator) vs `multitude::Arc`
//! (arena). Each is built with two strategies:
//!
//! - `*` — push freshly allocated properties through a growable vec, then
//!   freeze it into the `Arc`.
//! - `*_from_slice` — build directly from a pre-created slice of properties,
//!   with no intermediate vec.
//!
//! # Allocation hygiene
//!
//! Following the same toggle hygiene as `gungraun_alloc`: setup (the arena
//! warm-up, the payload, the pre-created property slice, and the pre-sized
//! output `Vec`) runs outside the callgrind toggle via `#[bench::run(...)]`.
//! The timed body only builds the structures and pushes the handles into the
//! pre-sized output `Vec`, which is returned by value so its `Drop` runs
//! outside the toggle. The only traffic counted is the build itself.

#![allow(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]
#![allow(clippy::type_complexity, reason = "benchmark state tuples are inherently complex")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]

use core::hint::black_box;
use std::sync::Arc as StdArc;

use gungraun::{Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};
use multitude::{Arc as ArenaArc, Arena};

// Array shape: `PROPERTIES` binary blobs of `PROPERTY_SIZE` bytes each, built
// `N` times per bench so the per-build instruction count is stable.
const PROPERTIES: usize = 8;
const PROPERTY_SIZE: usize = 16;
const N: usize = 1_000;

type GlobalArray = StdArc<[StdArc<[u8]>]>;
type ArenaArrayOfArena = ArenaArc<[ArenaArc<[u8]>]>;
type ArenaArrayOfGlobal = ArenaArc<[StdArc<[u8]>]>;

// ===== shared builders (mirror criterion_arc_array.rs) =====

fn build_global(payload: &[u8]) -> GlobalArray {
    let mut properties = Vec::with_capacity(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(StdArc::<[u8]>::from(payload));
    }
    StdArc::from(properties)
}

fn build_global_from_slice(properties: &[StdArc<[u8]>]) -> GlobalArray {
    StdArc::from(properties)
}

fn build_arena(arena: &Arena, payload: &[u8]) -> ArenaArrayOfArena {
    let mut properties = arena.alloc_vec_with_capacity::<ArenaArc<[u8]>>(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(arena.alloc_slice_copy_arc(payload));
    }
    properties.try_into_arc().unwrap()
}

fn build_arena_from_slice(arena: &Arena, properties: &[StdArc<[u8]>]) -> ArenaArrayOfGlobal {
    arena.alloc_slice_clone_arc(properties)
}

// ===== leaf setup helpers =====

fn payload() -> Vec<u8> {
    vec![0xAB_u8; PROPERTY_SIZE]
}

fn global_properties() -> Vec<StdArc<[u8]>> {
    let payload = payload();
    (0..PROPERTIES).map(|_| StdArc::<[u8]>::from(payload.as_slice())).collect()
}

fn warm_arena() -> Arena {
    // Warm: preallocate one chunk of the largest size class for each flavor
    // AND prime the arena's current_local / current_shared mutators with a
    // throwaway allocation, so the timed body never pays a cold `refill_*`.
    // Mirrors `gungraun_alloc::warm_arena`.
    let arena = Arena::builder()
        .with_capacity_local(64 * 1024)
        .with_capacity_shared(64 * 1024)
        .build();
    let _: &mut u64 = arena.alloc(0_u64);
    let _ = arena.alloc_arc(0_u64);
    arena
}

// ===== composite setups (pre-allocate the output Vec to N) =====

fn setup_global() -> (Vec<u8>, Vec<GlobalArray>) {
    (payload(), Vec::with_capacity(N))
}

fn setup_arena() -> (Arena, Vec<u8>, Vec<ArenaArrayOfArena>) {
    (warm_arena(), payload(), Vec::with_capacity(N))
}

fn setup_global_from_slice() -> (Vec<StdArc<[u8]>>, Vec<GlobalArray>) {
    (global_properties(), Vec::with_capacity(N))
}

fn setup_arena_from_slice() -> (Arena, Vec<StdArc<[u8]>>, Vec<ArenaArrayOfGlobal>) {
    (warm_arena(), global_properties(), Vec::with_capacity(N))
}

// ===== bench bodies — only the build is inside the toggle =====

#[library_benchmark]
#[bench::run(setup_global())]
fn global(state: (Vec<u8>, Vec<GlobalArray>)) -> (Vec<u8>, Vec<GlobalArray>) {
    let (payload, mut out) = state;
    for _ in 0..N {
        out.push(black_box(build_global(black_box(&payload))));
    }
    (payload, out)
}

#[library_benchmark]
#[bench::run(setup_arena())]
fn arena(state: (Arena, Vec<u8>, Vec<ArenaArrayOfArena>)) -> (Arena, Vec<u8>, Vec<ArenaArrayOfArena>) {
    let (arena, payload, mut out) = state;
    for _ in 0..N {
        out.push(black_box(build_arena(&arena, black_box(&payload))));
    }
    (arena, payload, out)
}

#[library_benchmark]
#[bench::run(setup_global_from_slice())]
fn global_from_slice(state: (Vec<StdArc<[u8]>>, Vec<GlobalArray>)) -> (Vec<StdArc<[u8]>>, Vec<GlobalArray>) {
    let (properties, mut out) = state;
    for _ in 0..N {
        out.push(black_box(build_global_from_slice(black_box(&properties))));
    }
    (properties, out)
}

#[library_benchmark]
#[bench::run(setup_arena_from_slice())]
fn arena_from_slice(state: (Arena, Vec<StdArc<[u8]>>, Vec<ArenaArrayOfGlobal>)) -> (Arena, Vec<StdArc<[u8]>>, Vec<ArenaArrayOfGlobal>) {
    let (arena, properties, mut out) = state;
    for _ in 0..N {
        out.push(black_box(build_arena_from_slice(&arena, black_box(&properties))));
    }
    (arena, properties, out)
}

library_benchmark_group!(
    name = arc_array_group;
    benchmarks = global, arena, global_from_slice, arena_from_slice
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = arc_array_group
);
