// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise drop benchmarks for multitude.
//!
//! Mirrors `benches/criterion_drop.rs` 1:1: each gungraun function
//! `drop_<variant>` corresponds to a criterion benchmark `drop/<variant>`.
//! Each setup pre-fills an arena with N handles; the bench body drops them
//! (handle vec + arena), measuring per-handle smart-pointer drop plus chunk
//! teardown at arena drop.

#![expect(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]

use core::hint::black_box;

use gungraun::{Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};
use multitude::strings::{ArcStr, BoxStr, RcStr};
use multitude::{Arc, Arena, Box, Rc};

const N: usize = 1_000;
const SLICE_LEN: usize = 8;

// `std::Box<u64>` is the `T: Drop` test type — its destructor calls into the
// global allocator, exercising the chunk drop-list traversal.
type DroppyT = std::boxed::Box<u64>;

#[expect(clippy::unnecessary_box_returns, reason = "Box<u64> is the T: Drop probe")]
fn make_droppy(i: usize) -> DroppyT {
    std::boxed::Box::new(i as u64)
}

// ===== single-value handle drops =====

fn setup_box_u64() -> (Vec<Box<u64>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_box(i as u64));
    }
    (h, arena)
}

fn setup_rc_u64() -> (Vec<Rc<u64>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_rc(i as u64));
    }
    (h, arena)
}

fn setup_arc_u64() -> (Vec<Arc<u64>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_arc(i as u64));
    }
    (h, arena)
}

fn setup_box_droppy() -> (Vec<Box<DroppyT>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_box(make_droppy(i)));
    }
    (h, arena)
}

fn setup_rc_droppy() -> (Vec<Rc<DroppyT>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_rc(make_droppy(i)));
    }
    (h, arena)
}

fn setup_arc_droppy() -> (Vec<Arc<DroppyT>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_arc(make_droppy(i)));
    }
    (h, arena)
}

// ===== str handle drops =====

fn setup_str_box() -> (Vec<BoxStr>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_str_box(format!("word{i}")));
    }
    (h, arena)
}

fn setup_str_rc() -> (Vec<RcStr>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_str_rc(format!("word{i}")));
    }
    (h, arena)
}

fn setup_str_arc() -> (Vec<ArcStr>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(arena.alloc_str_arc(format!("word{i}")));
    }
    (h, arena)
}

// ===== slice handle drops =====

fn setup_slice_box_u64() -> (Vec<Box<[u64]>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(arena.alloc_slice_fill_with_box::<u64, _>(SLICE_LEN, |j| j as u64));
    }
    (h, arena)
}

fn setup_slice_rc_u64() -> (Vec<Rc<[u64]>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(arena.alloc_slice_fill_with_rc::<u64, _>(SLICE_LEN, |j| j as u64));
    }
    (h, arena)
}

fn setup_slice_arc_u64() -> (Vec<Arc<[u64]>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(arena.alloc_slice_fill_with_arc::<u64, _>(SLICE_LEN, |j| j as u64));
    }
    (h, arena)
}

fn setup_slice_box_droppy() -> (Vec<Box<[DroppyT]>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(arena.alloc_slice_fill_with_box::<DroppyT, _>(SLICE_LEN, make_droppy));
    }
    (h, arena)
}

fn setup_slice_rc_droppy() -> (Vec<Rc<[DroppyT]>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(arena.alloc_slice_fill_with_rc::<DroppyT, _>(SLICE_LEN, make_droppy));
    }
    (h, arena)
}

fn setup_slice_arc_droppy() -> (Vec<Arc<[DroppyT]>>, Arena) {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(arena.alloc_slice_fill_with_arc::<DroppyT, _>(SLICE_LEN, make_droppy));
    }
    (h, arena)
}

// ===== arena-only drop (no handles) =====

fn setup_alloc() -> Arena {
    let arena = Arena::builder().with_capacity_local(64 * 1024).build();
    for i in 0..N {
        let _: &mut u64 = arena.alloc(i as u64);
    }
    arena
}

// ===== bench bodies — drop happens at scope exit =====

#[library_benchmark]
#[bench::run(setup_box_u64())]
fn drop_box_u64(state: (Vec<Box<u64>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_rc_u64())]
fn drop_rc_u64(state: (Vec<Rc<u64>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_arc_u64())]
fn drop_arc_u64(state: (Vec<Arc<u64>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_box_droppy())]
fn drop_box_droppy(state: (Vec<Box<DroppyT>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_rc_droppy())]
fn drop_rc_droppy(state: (Vec<Rc<DroppyT>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_arc_droppy())]
fn drop_arc_droppy(state: (Vec<Arc<DroppyT>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_str_box())]
fn drop_str_box(state: (Vec<BoxStr>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_str_rc())]
fn drop_str_rc(state: (Vec<RcStr>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_str_arc())]
fn drop_str_arc(state: (Vec<ArcStr>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_slice_box_u64())]
fn drop_slice_box_u64(state: (Vec<Box<[u64]>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_slice_rc_u64())]
fn drop_slice_rc_u64(state: (Vec<Rc<[u64]>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_slice_arc_u64())]
fn drop_slice_arc_u64(state: (Vec<Arc<[u64]>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_slice_box_droppy())]
fn drop_slice_box_droppy(state: (Vec<Box<[DroppyT]>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_slice_rc_droppy())]
fn drop_slice_rc_droppy(state: (Vec<Rc<[DroppyT]>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_slice_arc_droppy())]
fn drop_slice_arc_droppy(state: (Vec<Arc<[DroppyT]>>, Arena)) {
    black_box(state);
}

#[library_benchmark]
#[bench::run(setup_alloc())]
fn drop_alloc(state: Arena) {
    black_box(state);
}

library_benchmark_group!(
    name = drop_group;
    benchmarks =
        drop_box_u64, drop_rc_u64, drop_arc_u64,
        drop_box_droppy, drop_rc_droppy, drop_arc_droppy,
        drop_str_box, drop_str_rc, drop_str_arc,
        drop_slice_box_u64, drop_slice_rc_u64, drop_slice_arc_u64,
        drop_slice_box_droppy, drop_slice_rc_droppy, drop_slice_arc_droppy,
        drop_alloc
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = drop_group
);
