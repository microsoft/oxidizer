// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise allocation benchmarks for multitude.
//!
//! Mirrors `benches/criterion_alloc.rs` 1:1: each gungraun function
//! `<group>_<variant>` corresponds to a criterion benchmark
//! `<group>/<variant>`.
//!
//! Run with `cargo bench --bench gungraun_alloc` on a Linux host with Valgrind.
//!
//! # Allocation hygiene
//!
//! gungraun's callgrind toggle pattern is `*::__gungraun_wrapper_mod::*` —
//! collection turns ON when execution enters the wrapped bench fn and OFF
//! when it exits. That means EVERYTHING inside the bench fn body — including
//! the drop epilogue for by-value parameters and any `Vec::with_capacity`
//! call — is counted.
//!
//! To keep the timed region focused on the arena's allocation calls, every
//! bench fn here follows the same shape:
//!
//! 1. **Setup runs outside the toggle.** `#[bench::run(setup_*())]` returns
//!    an already-fully-allocated state tuple — including any output `Vec`
//!    pre-sized to `N` so the timed body never grows it.
//! 2. **Inputs flow through.** The bench takes its state by value and
//!    returns it by value. Rust moves these out at the return site; their
//!    `Drop` runs in `__gungraun_wrapper_id_mod::wrapper` (outside the
//!    toggle), not inside the bench fn.
//!
//! With this hygiene, the only system-allocator traffic counted is whatever
//! the arena call itself causes (which is the whole point of measuring).

#![allow(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]
#![allow(clippy::ref_as_ptr, reason = "trivial pointer cast in bench plumbing")]
#![allow(clippy::type_complexity, reason = "benchmark state tuples are inherently complex")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]

use core::hint::black_box;
use core::mem::MaybeUninit;

use gungraun::{Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};
use multitude::{Arc, Arena, Box};

const N: usize = 1_000;
const SLICE_LEN: usize = 8;

// ===== leaf setup helpers (call from composite setups below) =====

fn warm_bump() -> bumpalo::Bump {
    // Warm: force first-chunk allocation so the timed region exercises
    // only the warm-path bump cursor, not the cold-create cliff.
    let bump = bumpalo::Bump::with_capacity(64 * 1024);
    let _: &mut u64 = bump.alloc(0_u64);
    bump
}

fn warm_arena() -> Arena {
    // Warm: preallocate chunks of the largest size class AND prime the
    // arena's `current` mutator by performing a throwaway reference
    // allocation and a throwaway `Arc` allocation.
    // The preallocated chunks live in the provider cache; the
    // `current` slot starts in the empty-mutator state and is only
    // populated lazily on the first allocation. Without the prime,
    // every bench fn entry would pay one cold `refill` (chunk-cache
    // pop + mutator install) on the first inner iteration, hiding
    // ~50 cold-path instructions inside the per-op instruction count
    // and adding cache-miss latency that doesn't reflect steady-state
    // performance. This mirrors bumpalo's `warm_bump` (which itself
    // primes its cursor with a no-op alloc).
    let arena = Arena::builder().with_capacity(128 * 1024).build();
    let _: &mut u64 = arena.alloc(0_u64);
    let _ = arena.alloc_arc(0_u64);
    arena
}

fn word_inputs() -> Vec<String> {
    (0..N).map(|i| format!("word{i}")).collect()
}

fn int_inputs() -> Vec<i32> {
    (0..N).map(|i| i32::try_from(i).unwrap_or(0)).collect()
}

fn slice_inputs() -> Vec<[u64; SLICE_LEN]> {
    (0..N)
        .map(|i| {
            let base = i as u64;
            [base, base + 1, base + 2, base + 3, base + 4, base + 5, base + 6, base + 7]
        })
        .collect()
}

// ===== composite setups (one per bench shape; pre-allocate output Vec) =====
//
// Each helper returns the full state tuple the bench will consume by value
// and return by value. The output `Vec<T>` is pre-sized to `N` so the
// timed region never allocates a backing buffer; `T` is inferred from the
// bench fn's parameter type.

fn arena_out<T>() -> (Arena, Vec<T>) {
    (warm_arena(), Vec::with_capacity(N))
}

fn arena_words_out<T>() -> (Arena, Vec<String>, Vec<T>) {
    (warm_arena(), word_inputs(), Vec::with_capacity(N))
}

fn arena_slices_out<T>() -> (Arena, Vec<[u64; SLICE_LEN]>, Vec<T>) {
    (warm_arena(), slice_inputs(), Vec::with_capacity(N))
}

fn arena_words() -> (Arena, Vec<String>) {
    (warm_arena(), word_inputs())
}

fn arena_ints() -> (Arena, Vec<i32>) {
    (warm_arena(), int_inputs())
}

fn arena_slices() -> (Arena, Vec<[u64; SLICE_LEN]>) {
    (warm_arena(), slice_inputs())
}

fn bump_words_out<T>() -> (bumpalo::Bump, Vec<String>, Vec<T>) {
    (warm_bump(), word_inputs(), Vec::with_capacity(N))
}

fn bump_words() -> (bumpalo::Bump, Vec<String>) {
    (warm_bump(), word_inputs())
}

fn bump_ints() -> (bumpalo::Bump, Vec<i32>) {
    (warm_bump(), int_inputs())
}

fn bump_slices() -> (bumpalo::Bump, Vec<[u64; SLICE_LEN]>) {
    (warm_bump(), slice_inputs())
}

// ===== alloc_u64: single-value allocation of u64 =====

#[library_benchmark]
#[bench::run(warm_arena())]
fn alloc(arena: Arena) -> Arena {
    for i in 0..N {
        let _: &mut u64 = black_box(arena.alloc(black_box(i as u64)));
    }
    arena
}

#[library_benchmark]
#[bench::run(warm_arena())]
fn alloc_with(arena: Arena) -> Arena {
    for i in 0..N {
        let _: &mut u64 = black_box(arena.alloc_with(|| black_box(i as u64)));
    }
    arena
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_box(state: (Arena, Vec<Box<u64>>)) -> (Arena, Vec<Box<u64>>) {
    let (arena, mut out) = state;
    for i in 0..N {
        out.push(black_box(arena.alloc_box(black_box(i as u64))));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_box_with(state: (Arena, Vec<Box<u64>>)) -> (Arena, Vec<Box<u64>>) {
    let (arena, mut out) = state;
    for i in 0..N {
        out.push(black_box(arena.alloc_box_with(|| black_box(i as u64))));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_uninit_box(state: (Arena, Vec<Box<MaybeUninit<u64>>>)) -> (Arena, Vec<Box<MaybeUninit<u64>>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_box::<u64>()));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_zeroed_box(state: (Arena, Vec<Box<MaybeUninit<u64>>>)) -> (Arena, Vec<Box<MaybeUninit<u64>>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_box::<u64>()));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_arc(state: (Arena, Vec<Arc<u64>>)) -> (Arena, Vec<Arc<u64>>) {
    let (arena, mut out) = state;
    for i in 0..N {
        out.push(black_box(arena.alloc_arc(black_box(i as u64))));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_arc_with(state: (Arena, Vec<Arc<u64>>)) -> (Arena, Vec<Arc<u64>>) {
    let (arena, mut out) = state;
    for i in 0..N {
        out.push(black_box(arena.alloc_arc_with(|| black_box(i as u64))));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_uninit_arc(state: (Arena, Vec<Arc<MaybeUninit<u64>>>)) -> (Arena, Vec<Arc<MaybeUninit<u64>>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_arc::<u64>()));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_zeroed_arc(state: (Arena, Vec<Arc<MaybeUninit<u64>>>)) -> (Arena, Vec<Arc<MaybeUninit<u64>>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_arc::<u64>()));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(warm_bump())]
fn bumpalo_alloc(bump: bumpalo::Bump) -> bumpalo::Bump {
    for i in 0..N {
        let _: &mut u64 = black_box(bump.alloc(black_box(i as u64)));
    }
    bump
}

#[library_benchmark]
#[bench::run(warm_bump())]
fn bumpalo_alloc_with(bump: bumpalo::Bump) -> bumpalo::Bump {
    for i in 0..N {
        let _: &mut u64 = black_box(bump.alloc_with(|| black_box(i as u64)));
    }
    bump
}

// ===== alloc_str: single &str allocation =====

#[library_benchmark]
#[bench::run(arena_words_out())]
fn alloc_str(state: (Arena, Vec<String>, Vec<*mut str>)) -> (Arena, Vec<String>, Vec<*mut str>) {
    let (arena, words, mut out) = state;
    for w in &words {
        let s: &mut str = black_box(arena.alloc_str(black_box(w.as_str())));
        out.push(s as *mut str);
    }
    (arena, words, out)
}

#[library_benchmark]
#[bench::run(arena_words_out())]
fn alloc_str_box(state: (Arena, Vec<String>, Vec<Box<str>>)) -> (Arena, Vec<String>, Vec<Box<str>>) {
    let (arena, words, mut out) = state;
    for w in &words {
        out.push(black_box(arena.alloc_str_box(black_box(w.as_str()))));
    }
    (arena, words, out)
}

#[library_benchmark]
#[bench::run(arena_words_out())]
fn alloc_str_arc(state: (Arena, Vec<String>, Vec<Arc<str>>)) -> (Arena, Vec<String>, Vec<Arc<str>>) {
    let (arena, words, mut out) = state;
    for w in &words {
        out.push(black_box(arena.alloc_str_arc(black_box(w.as_str()))));
    }
    (arena, words, out)
}

#[library_benchmark]
#[bench::run(bump_words_out())]
fn bumpalo_alloc_str(state: (bumpalo::Bump, Vec<String>, Vec<*mut str>)) -> (bumpalo::Bump, Vec<String>, Vec<*mut str>) {
    let (bump, words, mut out) = state;
    for w in &words {
        let s: &mut str = black_box(bump.alloc_str(black_box(w.as_str())));
        out.push(s as *mut str);
    }
    (bump, words, out)
}

// ===== alloc_slice: slice<u64>, len = SLICE_LEN, N batches =====

#[library_benchmark]
#[bench::run(arena_slices())]
fn alloc_slice_copy(state: (Arena, Vec<[u64; SLICE_LEN]>)) -> (Arena, Vec<[u64; SLICE_LEN]>) {
    let (arena, slices) = state;
    for s in &slices {
        let _: &mut [u64] = black_box(arena.alloc_slice_copy(black_box(s.as_slice())));
    }
    (arena, slices)
}

#[library_benchmark]
#[bench::run(arena_slices())]
fn alloc_slice_clone(state: (Arena, Vec<[u64; SLICE_LEN]>)) -> (Arena, Vec<[u64; SLICE_LEN]>) {
    let (arena, slices) = state;
    for s in &slices {
        let _: &mut [u64] = black_box(arena.alloc_slice_clone(black_box(s.as_slice())));
    }
    (arena, slices)
}

#[library_benchmark]
#[bench::run(warm_arena())]
fn alloc_slice_fill_with(arena: Arena) -> Arena {
    for _ in 0..N {
        let _: &mut [u64] = black_box(arena.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
    }
    arena
}

#[library_benchmark]
#[bench::run(warm_arena())]
fn alloc_slice_fill_iter(arena: Arena) -> Arena {
    for _ in 0..N {
        let _: &mut [u64] = black_box(arena.alloc_slice_fill_iter((0..SLICE_LEN).map(|j| black_box(j as u64))));
    }
    arena
}

// box variants

#[library_benchmark]
#[bench::run(arena_slices_out())]
fn alloc_slice_copy_box(state: (Arena, Vec<[u64; SLICE_LEN]>, Vec<Box<[u64]>>)) -> (Arena, Vec<[u64; SLICE_LEN]>, Vec<Box<[u64]>>) {
    let (arena, slices, mut out) = state;
    for s in &slices {
        out.push(black_box(arena.alloc_slice_copy_box(black_box(s.as_slice()))));
    }
    (arena, slices, out)
}

#[library_benchmark]
#[bench::run(arena_slices_out())]
fn alloc_slice_clone_box(state: (Arena, Vec<[u64; SLICE_LEN]>, Vec<Box<[u64]>>)) -> (Arena, Vec<[u64; SLICE_LEN]>, Vec<Box<[u64]>>) {
    let (arena, slices, mut out) = state;
    for s in &slices {
        out.push(black_box(arena.alloc_slice_clone_box(black_box(s.as_slice()))));
    }
    (arena, slices, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_slice_fill_with_box(state: (Arena, Vec<Box<[u64]>>)) -> (Arena, Vec<Box<[u64]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_with_box::<u64, _>(SLICE_LEN, |j| black_box(j as u64)),
        ));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_slice_fill_iter_box(state: (Arena, Vec<Box<[u64]>>)) -> (Arena, Vec<Box<[u64]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_iter_box((0..SLICE_LEN).map(|j| black_box(j as u64))),
        ));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_uninit_slice_box(state: (Arena, Vec<Box<[MaybeUninit<u64>]>>)) -> (Arena, Vec<Box<[MaybeUninit<u64>]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_slice_box::<u64>(SLICE_LEN)));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_zeroed_slice_box(state: (Arena, Vec<Box<[MaybeUninit<u64>]>>)) -> (Arena, Vec<Box<[MaybeUninit<u64>]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_slice_box::<u64>(SLICE_LEN)));
    }
    (arena, out)
}

// arc variants

#[library_benchmark]
#[bench::run(arena_slices_out())]
fn alloc_slice_copy_arc(state: (Arena, Vec<[u64; SLICE_LEN]>, Vec<Arc<[u64]>>)) -> (Arena, Vec<[u64; SLICE_LEN]>, Vec<Arc<[u64]>>) {
    let (arena, slices, mut out) = state;
    for s in &slices {
        out.push(black_box(arena.alloc_slice_copy_arc(black_box(s.as_slice()))));
    }
    (arena, slices, out)
}

#[library_benchmark]
#[bench::run(arena_slices_out())]
fn alloc_slice_clone_arc(state: (Arena, Vec<[u64; SLICE_LEN]>, Vec<Arc<[u64]>>)) -> (Arena, Vec<[u64; SLICE_LEN]>, Vec<Arc<[u64]>>) {
    let (arena, slices, mut out) = state;
    for s in &slices {
        out.push(black_box(arena.alloc_slice_clone_arc(black_box(s.as_slice()))));
    }
    (arena, slices, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_slice_fill_with_arc(state: (Arena, Vec<Arc<[u64]>>)) -> (Arena, Vec<Arc<[u64]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_with_arc::<u64, _>(SLICE_LEN, |j| black_box(j as u64)),
        ));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_slice_fill_iter_arc(state: (Arena, Vec<Arc<[u64]>>)) -> (Arena, Vec<Arc<[u64]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_iter_arc((0..SLICE_LEN).map(|j| black_box(j as u64))),
        ));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_uninit_slice_arc(state: (Arena, Vec<Arc<[MaybeUninit<u64>]>>)) -> (Arena, Vec<Arc<[MaybeUninit<u64>]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_slice_arc::<u64>(SLICE_LEN)));
    }
    (arena, out)
}

#[library_benchmark]
#[bench::run(arena_out())]
fn alloc_zeroed_slice_arc(state: (Arena, Vec<Arc<[MaybeUninit<u64>]>>)) -> (Arena, Vec<Arc<[MaybeUninit<u64>]>>) {
    let (arena, mut out) = state;
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_slice_arc::<u64>(SLICE_LEN)));
    }
    (arena, out)
}

// bumpalo slice variants

#[library_benchmark]
#[bench::run(bump_slices())]
fn bumpalo_alloc_slice_copy(state: (bumpalo::Bump, Vec<[u64; SLICE_LEN]>)) -> (bumpalo::Bump, Vec<[u64; SLICE_LEN]>) {
    let (bump, slices) = state;
    for s in &slices {
        let _: &mut [u64] = black_box(bump.alloc_slice_copy(black_box(s.as_slice())));
    }
    (bump, slices)
}

#[library_benchmark]
#[bench::run(bump_slices())]
fn bumpalo_alloc_slice_clone(state: (bumpalo::Bump, Vec<[u64; SLICE_LEN]>)) -> (bumpalo::Bump, Vec<[u64; SLICE_LEN]>) {
    let (bump, slices) = state;
    for s in &slices {
        let _: &mut [u64] = black_box(bump.alloc_slice_clone(black_box(s.as_slice())));
    }
    (bump, slices)
}

#[library_benchmark]
#[bench::run(warm_bump())]
fn bumpalo_alloc_slice_fill_with(bump: bumpalo::Bump) -> bumpalo::Bump {
    for _ in 0..N {
        let _: &mut [u64] = black_box(bump.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
    }
    bump
}

#[library_benchmark]
#[bench::run(warm_bump())]
fn bumpalo_alloc_slice_fill_iter(bump: bumpalo::Bump) -> bumpalo::Bump {
    for _ in 0..N {
        let _: &mut [u64] = black_box(bump.alloc_slice_fill_iter((0..SLICE_LEN).map(|j| black_box(j as u64))));
    }
    bump
}

// ===== string_builder: push N tokens, freeze =====

#[library_benchmark]
#[bench::run(arena_words())]
fn alloc_string(state: (Arena, Vec<String>)) -> (*const str, Arena, Vec<String>) {
    let (arena, words) = state;
    let mut s = arena.alloc_string();
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    // Mirror bumpalo's `into_bump_str`: take a `&str` view of the in-place
    // chunk storage with no copy into a `Box<str>`. The bytes stay valid
    // until arena teardown, so the returned pointer remains usable.
    let frozen: *const str = black_box(s.as_str() as *const str);
    drop(s);
    (frozen, arena, words)
}

#[library_benchmark]
#[bench::run(arena_words())]
fn alloc_string_with_capacity(state: (Arena, Vec<String>)) -> (*const str, Arena, Vec<String>) {
    let (arena, words) = state;
    let mut s = arena.alloc_string_with_capacity(N * 6);
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen: *const str = black_box(s.as_str() as *const str);
    drop(s);
    (frozen, arena, words)
}

#[library_benchmark]
#[bench::run(bump_words())]
fn bumpalo_string_new_in(state: (bumpalo::Bump, Vec<String>)) -> (*const str, bumpalo::Bump, Vec<String>) {
    let (bump, words) = state;
    let mut s = bumpalo::collections::String::new_in(&bump);
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen: &str = black_box(s.into_bump_str());
    (frozen as *const str, bump, words)
}

#[library_benchmark]
#[bench::run(bump_words())]
fn bumpalo_string_with_capacity_in(state: (bumpalo::Bump, Vec<String>)) -> (*const str, bumpalo::Bump, Vec<String>) {
    let (bump, words) = state;
    let mut s = bumpalo::collections::String::with_capacity_in(N * 6, &bump);
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen: &str = black_box(s.into_bump_str());
    (frozen as *const str, bump, words)
}

// ===== vec_builder: push N i32, freeze =====

#[library_benchmark]
#[bench::run(arena_ints())]
fn alloc_vec(state: (Arena, Vec<i32>)) -> (*const [i32], Arena, Vec<i32>) {
    let (arena, ints) = state;
    let mut v = arena.alloc_vec::<i32>();
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen: *const [i32] = black_box(v.leak() as *const [i32]);
    (frozen, arena, ints)
}

#[library_benchmark]
#[bench::run(arena_ints())]
fn alloc_vec_with_capacity(state: (Arena, Vec<i32>)) -> (*const [i32], Arena, Vec<i32>) {
    let (arena, ints) = state;
    let mut v = arena.alloc_vec_with_capacity::<i32>(N);
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen: *const [i32] = black_box(v.leak() as *const [i32]);
    (frozen, arena, ints)
}

#[library_benchmark]
#[bench::run(bump_ints())]
fn bumpalo_vec_new_in(state: (bumpalo::Bump, Vec<i32>)) -> (*const [i32], bumpalo::Bump, Vec<i32>) {
    let (bump, ints) = state;
    let mut v: bumpalo::collections::Vec<'_, i32> = bumpalo::collections::Vec::new_in(&bump);
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen: &[i32] = black_box(v.into_bump_slice());
    (frozen as *const [i32], bump, ints)
}

#[library_benchmark]
#[bench::run(bump_ints())]
fn bumpalo_vec_with_capacity_in(state: (bumpalo::Bump, Vec<i32>)) -> (*const [i32], bumpalo::Bump, Vec<i32>) {
    let (bump, ints) = state;
    let mut v: bumpalo::collections::Vec<'_, i32> = bumpalo::collections::Vec::with_capacity_in(N, &bump);
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen: &[i32] = black_box(v.into_bump_slice());
    (frozen as *const [i32], bump, ints)
}

// ===== arena_creation: standalone Arena/Bump construction + drop =====
//
// EXEMPT from the no-syscall-in-timed-region policy: these benches
// specifically measure the cost of arena construction, which intrinsically
// involves a system allocation.

#[library_benchmark]
fn multitude_new() {
    let arena = black_box(Arena::new());
    drop(arena);
}

#[library_benchmark]
fn bumpalo_new() {
    let bump = black_box(bumpalo::Bump::new());
    drop(bump);
}

library_benchmark_group!(
    name = alloc_group;
    benchmarks =
        multitude_new, bumpalo_new,
        alloc, alloc_with,
        alloc_box, alloc_box_with,
        alloc_uninit_box, alloc_zeroed_box,
        alloc_arc, alloc_arc_with,
        alloc_uninit_arc, alloc_zeroed_arc,
        bumpalo_alloc, bumpalo_alloc_with,
        alloc_str, alloc_str_box,
        alloc_str_arc, bumpalo_alloc_str,
        alloc_slice_copy, alloc_slice_clone,
        alloc_slice_fill_with, alloc_slice_fill_iter,
        alloc_slice_copy_box, alloc_slice_clone_box,
        alloc_slice_fill_with_box, alloc_slice_fill_iter_box,
        alloc_uninit_slice_box, alloc_zeroed_slice_box,
        alloc_slice_copy_arc, alloc_slice_clone_arc,
        alloc_slice_fill_with_arc, alloc_slice_fill_iter_arc,
        alloc_uninit_slice_arc, alloc_zeroed_slice_arc,
        bumpalo_alloc_slice_copy, bumpalo_alloc_slice_clone,
        bumpalo_alloc_slice_fill_with, bumpalo_alloc_slice_fill_iter,
        alloc_string, alloc_string_with_capacity,
        bumpalo_string_new_in, bumpalo_string_with_capacity_in,
        alloc_vec, alloc_vec_with_capacity,
        bumpalo_vec_new_in, bumpalo_vec_with_capacity_in
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = alloc_group
);
