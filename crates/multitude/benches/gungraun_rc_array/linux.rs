// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise `Rc<[Rc<[u8]>]>` build benchmarks for multitude.
//!
//! Mirrors `benches/criterion_rc_array.rs` 1:1: each gungraun function
//! `<variant>` corresponds to a criterion benchmark `rc_array/<variant>`.
//! Builds an `Rc<[Rc<[u8]>]>` of `PROPERTIES` binary blobs two ways and
//! compares them: `std::rc::Rc` (global allocator) vs `multitude::Rc`
//! (arena). Each is built with two strategies:
//!
//! - `*` — push freshly allocated properties through a growable vec, then
//!   freeze it into the `Rc`.
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
use std::rc::Rc as StdRc;

use gungraun::{Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};
use multitude::{Arena, Rc as ArenaRc};

// Array shape: `PROPERTIES` binary blobs of `PROPERTY_SIZE` bytes each, built
// `N` times per bench so the per-build instruction count is stable.
const PROPERTIES: usize = 8;
const PROPERTY_SIZE: usize = 16;
const N: usize = 1_000;

type GlobalArray = StdRc<[StdRc<[u8]>]>;
type ArenaArrayOfArena = ArenaRc<[ArenaRc<[u8]>]>;
type ArenaArrayOfGlobal = ArenaRc<[StdRc<[u8]>]>;

// ===== shared builders (mirror criterion_rc_array.rs) =====

fn build_global(payload: &[u8]) -> GlobalArray {
    let mut properties = Vec::with_capacity(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(StdRc::<[u8]>::from(payload));
    }
    StdRc::from(properties)
}

fn build_global_from_slice(properties: &[StdRc<[u8]>]) -> GlobalArray {
    StdRc::from(properties)
}

fn build_arena(arena: &Arena, payload: &[u8]) -> ArenaArrayOfArena {
    let mut properties = arena.alloc_vec_with_capacity::<ArenaRc<[u8]>>(PROPERTIES);
    for _ in 0..PROPERTIES {
        properties.push(arena.alloc_slice_copy_rc(payload));
    }
    properties.try_into_rc_slice().unwrap()
}

fn build_arena_from_slice(arena: &Arena, properties: &[StdRc<[u8]>]) -> ArenaArrayOfGlobal {
    arena.alloc_slice_clone_rc(properties)
}

// ===== leaf setup helpers =====

fn payload() -> Vec<u8> {
    vec![0xAB_u8; PROPERTY_SIZE]
}

fn global_properties() -> Vec<StdRc<[u8]>> {
    let payload = payload();
    (0..PROPERTIES).map(|_| StdRc::<[u8]>::from(payload.as_slice())).collect()
}

fn warm_arena() -> Arena {
    // Warm: preallocate chunks of the largest size class AND prime the
    // arena's `current` mutator with a throwaway reference allocation
    // and a throwaway `Rc` allocation, so the timed body never pays a
    // cold `refill`. Mirrors `gungraun_alloc::warm_arena`.
    let arena = Arena::builder().with_capacity(128 * 1024).build();
    let _ = arena.alloc(0_u64);
    let _ = arena.alloc_rc(0_u64);
    arena
}

// ===== composite setups (pre-allocate the output Vec to N) =====

fn setup_global() -> (Vec<u8>, Vec<GlobalArray>) {
    (payload(), Vec::with_capacity(N))
}

fn setup_arena() -> (Arena, Vec<u8>, Vec<ArenaArrayOfArena>) {
    (warm_arena(), payload(), Vec::with_capacity(N))
}

fn setup_global_from_slice() -> (Vec<StdRc<[u8]>>, Vec<GlobalArray>) {
    (global_properties(), Vec::with_capacity(N))
}

fn setup_arena_from_slice() -> (Arena, Vec<StdRc<[u8]>>, Vec<ArenaArrayOfGlobal>) {
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
fn global_from_slice(state: (Vec<StdRc<[u8]>>, Vec<GlobalArray>)) -> (Vec<StdRc<[u8]>>, Vec<GlobalArray>) {
    let (properties, mut out) = state;
    for _ in 0..N {
        out.push(black_box(build_global_from_slice(black_box(&properties))));
    }
    (properties, out)
}

#[library_benchmark]
#[bench::run(setup_arena_from_slice())]
fn arena_from_slice(state: (Arena, Vec<StdRc<[u8]>>, Vec<ArenaArrayOfGlobal>)) -> (Arena, Vec<StdRc<[u8]>>, Vec<ArenaArrayOfGlobal>) {
    let (arena, properties, mut out) = state;
    for _ in 0..N {
        out.push(black_box(build_arena_from_slice(&arena, black_box(&properties))));
    }
    (arena, properties, out)
}

library_benchmark_group!(
    name = rc_array_group;
    benchmarks = global, arena, global_from_slice, arena_from_slice
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = rc_array_group
);
