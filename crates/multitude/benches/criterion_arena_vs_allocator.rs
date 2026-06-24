// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-lifecycle benchmark: allocate a bunch of objects and then release
//! them all, comparing a `multitude` arena against the system allocator
//! ([mimalloc](https://github.com/microsoft/mimalloc), installed as this
//! bench's global allocator).
//!
//! Each variant performs the **exact same allocation work** — `N` units, each
//! allocating a `u64`, a fixed-size byte slice, and a short string — and then
//! releases everything:
//!
//! - `arena` — bump-allocate every object **and** the `Vec`s that hold the
//!   owning handles into one warmed-up [`Arena`], then [`reset`](Arena::reset)
//!   it. Every allocation in the timed region comes from the arena; release
//!   rewinds the bump cursor, reclaiming everything at once and retaining the
//!   chunks for the next iteration — no system-allocator traffic at all.
//! - `system` — allocate the exact same object types (`u64`, `[u8]`, `str`) as
//!   `Box`es, held in the same `Vec` spines, all from the global allocator
//!   (mimalloc), then drop them. Release pays one free per object.
//!
//! The arena is created and warmed once outside the timed region (one workload
//! pass grows it to hold the full working set). The timed region then covers
//! all allocations and release. The source words and payload are built once
//! outside the timed region so only allocation and free traffic is measured.
//!
//! Run with: `cargo bench --bench criterion_arena_vs_allocator`
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(unused_results, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use multitude::{Alloc, Arena};

/// Use mimalloc as the system allocator so the `system` variant (and the
/// arena's own chunk allocations) measure against a high-performance allocator
/// rather than the platform default.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Workload shape: `N` units, each a `u64`, a `SLICE_LEN`-byte slice, and a
// short string — a representative mix of "a bunch of stuff".
const N: usize = 1_000;
const SLICE_LEN: usize = 32;

/// Allocate the full working set **entirely from `arena`**: every object *and*
/// the `Vec`s that hold the owning handles come from the arena, so no
/// system-allocator traffic occurs here. The handles are dropped when this
/// returns, so the caller can then `reset`.
fn alloc_arena_workload(arena: &Arena, words: &[String], payload: &[u8]) {
    let mut vals = arena.alloc_vec_with_capacity::<Alloc<'_, u64>>(N);
    let mut slices = arena.alloc_vec_with_capacity::<Alloc<'_, [u8]>>(N);
    let mut strs = arena.alloc_vec_with_capacity::<Alloc<'_, str>>(N);
    for (i, w) in words.iter().enumerate() {
        vals.push(arena.alloc(black_box(i as u64)));
        slices.push(arena.alloc_slice_copy(black_box(payload)));
        strs.push(arena.alloc_str(black_box(w.as_str())));
    }
    black_box((&vals, &slices, &strs));
}

/// System-allocator mirror of [`alloc_arena_workload`]: the exact same object
/// types (`u64`, `[u8]`, `str`) and the same `Vec` spines, but every allocation
/// comes from the global allocator (mimalloc). Everything is freed (per object)
/// when this returns.
fn alloc_system_workload(words: &[String], payload: &[u8]) {
    let mut vals: Vec<Box<u64>> = Vec::with_capacity(N);
    let mut slices: Vec<Box<[u8]>> = Vec::with_capacity(N);
    let mut strs: Vec<Box<str>> = Vec::with_capacity(N);
    for (i, w) in words.iter().enumerate() {
        vals.push(Box::new(black_box(i as u64)));
        slices.push(Box::<[u8]>::from(black_box(payload)));
        strs.push(Box::<str>::from(black_box(w.as_str())));
    }
    black_box((&vals, &slices, &strs));
}

fn bench_arena_vs_allocator(c: &mut Criterion) {
    // Built once, outside the timed region: only allocation/free is measured.
    let words: Vec<String> = (0..N).map(|i| format!("item-{i:08}")).collect();
    let payload = [0xAB_u8; SLICE_LEN];

    let mut g = c.benchmark_group("arena_vs_allocator");

    g.bench_function("arena", |b| {
        let mut arena = Arena::new();
        // Warm up outside the timed region: repeat the workload enough times to
        // ratchet the arena's chunk size-classes to steady state and grow it to
        // hold the full working set, so the timed iterations reuse those chunks
        // rather than paying cold first-chunk system allocations.
        for _ in 0..32 {
            alloc_arena_workload(&arena, &words, payload.as_slice());
            arena.reset();
        }
        b.iter(|| {
            alloc_arena_workload(&arena, &words, black_box(payload.as_slice()));
            // Release: reset rewinds the bump cursor, reclaiming every
            // allocation at once and retaining the chunks for reuse.
            arena.reset();
        });
    });

    g.bench_function("system", |b| {
        b.iter(|| {
            // Release is the per-object free that happens when the workload's
            // `Vec`s and `Box`es drop at the end of each call.
            alloc_system_workload(&words, black_box(payload.as_slice()));
        });
    });

    g.finish();
}

criterion_group!(benches, bench_arena_vs_allocator);
criterion_main!(benches);
