// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Linux Callgrind `Rc<[Rc<[u8]>]>` build benchmarks for multitude.
//!
//! Paired with `criterion_rc_array.rs`.
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
//! Following the same toggle hygiene as `criterion_alloc_cg`: setup (the arena
//! warm-up, the payload, and the pre-created property slice) runs outside the
//! callgrind toggle via `#[bench::run(...)]`. Each timed body performs exactly
//! one build, matching its Criterion counterpart, and returns all state so
//! teardown runs outside the toggle.

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

use gungraun::{library_benchmark, library_benchmark_group};
use multitude::{Arena, Rc as ArenaRc};

// Array shape: `PROPERTIES` binary blobs of `PROPERTY_SIZE` bytes each.
const PROPERTIES: usize = 8;
const PROPERTY_SIZE: usize = 16;

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
    // cold `refill`. Mirrors `criterion_alloc_cg::warm_arena`.
    let arena = Arena::builder().with_capacity(128 * 1024).build();
    let _ = arena.alloc(0_u64);
    let _ = arena.alloc_rc(0_u64);
    arena
}

// ===== composite setups =====

fn setup_global() -> Vec<u8> {
    payload()
}

fn setup_arena() -> (Arena, Vec<u8>) {
    (warm_arena(), payload())
}

fn setup_global_from_slice() -> Vec<StdRc<[u8]>> {
    global_properties()
}

fn setup_arena_from_slice() -> (Arena, Vec<StdRc<[u8]>>) {
    (warm_arena(), global_properties())
}

// ===== bench bodies — one build inside the toggle =====

#[library_benchmark]
#[bench::run(setup_global())]
fn rc_array_global(payload: Vec<u8>) -> (GlobalArray, Vec<u8>) {
    let result = black_box(build_global(black_box(&payload)));
    (result, payload)
}

#[library_benchmark]
#[bench::run(setup_arena())]
fn rc_array_arena(state: (Arena, Vec<u8>)) -> (ArenaArrayOfArena, Arena, Vec<u8>) {
    let (arena, payload) = state;
    let result = black_box(build_arena(&arena, black_box(&payload)));
    (result, arena, payload)
}

#[library_benchmark]
#[bench::run(setup_global_from_slice())]
fn rc_array_global_from_slice(properties: Vec<StdRc<[u8]>>) -> (GlobalArray, Vec<StdRc<[u8]>>) {
    let result = black_box(build_global_from_slice(black_box(&properties)));
    (result, properties)
}

#[library_benchmark]
#[bench::run(setup_arena_from_slice())]
fn rc_array_arena_from_slice(state: (Arena, Vec<StdRc<[u8]>>)) -> (ArenaArrayOfGlobal, Arena, Vec<StdRc<[u8]>>) {
    let (arena, properties) = state;
    let result = black_box(build_arena_from_slice(&arena, black_box(&properties)));
    (result, arena, properties)
}

library_benchmark_group!(
    name = rc_array;
    benchmarks = rc_array_global, rc_array_arena, rc_array_global_from_slice, rc_array_arena_from_slice
);
