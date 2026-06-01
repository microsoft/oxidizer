// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group};
use multitude::Arena;

const N: usize = 1_000;
const SLICE_LEN: usize = 8;

// ===== setup helpers =====

fn setup_bumpalo() -> bumpalo::Bump {
    // Warm: force first-chunk allocation so the timed region exercises
    // only the warm-path bump cursor, not the cold-create cliff.
    let bump = bumpalo::Bump::with_capacity(64 * 1024);
    let _: &mut u64 = bump.alloc(0_u64);
    bump
}

fn setup_multitude() -> Arena {
    // Warm: preallocate one chunk of the largest size class for each
    // flavor so the timed region exercises only the warm-path bump
    // cursor, not the cold-create cliff. Both flavors are seeded
    // because gungraun runs Arc-flavor benches (`alloc_arc`,
    // `alloc_*_arc`, `alloc_slice_*_arc`) against the same shared
    // setup; without `with_capacity_shared`, those benches would pay
    // for a fresh shared chunk allocation on the first iteration.
    Arena::builder()
        .with_capacity_local(64 * 1024)
        .with_capacity_shared(64 * 1024)
        .build()
}

fn setup_word_inputs() -> Vec<String> {
    (0..N).map(|i| format!("word{i}")).collect()
}

fn setup_int_inputs() -> Vec<i32> {
    (0..N).map(|i| i32::try_from(i).unwrap_or(0)).collect()
}

fn setup_slice_inputs() -> Vec<[u64; SLICE_LEN]> {
    (0..N)
        .map(|i| {
            let base = i as u64;
            [base, base + 1, base + 2, base + 3, base + 4, base + 5, base + 6, base + 7]
        })
        .collect()
}

// ===== alloc_u64: single-value allocation of u64 =====

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc(arena: Arena) -> Arena {
    for i in 0..N {
        let _: &mut u64 = black_box(black_box(&arena).alloc(black_box(i as u64)));
    }
    arena
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_with(arena: Arena) -> Arena {
    for i in 0..N {
        let _: &mut u64 = black_box(black_box(&arena).alloc_with(|| black_box(i as u64)));
    }
    arena
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_box(arena: Arena) -> (Vec<multitude::Box<u64>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(black_box(arena.alloc_box(black_box(i as u64))));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_box_with(arena: Arena) -> (Vec<multitude::Box<u64>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(black_box(arena.alloc_box_with(|| black_box(i as u64))));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_uninit_box(arena: Arena) -> (Vec<multitude::Box<core::mem::MaybeUninit<u64>>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(black_box(arena.alloc_uninit_box::<u64>()));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_zeroed_box(arena: Arena) -> (Vec<multitude::Box<core::mem::MaybeUninit<u64>>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(black_box(arena.alloc_zeroed_box::<u64>()));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_rc(arena: Arena) -> (Vec<multitude::Rc<u64>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(black_box(arena.alloc_rc(black_box(i as u64))));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_rc_with(arena: Arena) -> (Vec<multitude::Rc<u64>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(black_box(arena.alloc_rc_with(|| black_box(i as u64))));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_uninit_rc(arena: Arena) -> (Vec<multitude::Rc<core::mem::MaybeUninit<u64>>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(black_box(arena.alloc_uninit_rc::<u64>()));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_zeroed_rc(arena: Arena) -> (Vec<multitude::Rc<core::mem::MaybeUninit<u64>>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(black_box(arena.alloc_zeroed_rc::<u64>()));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_arc(arena: Arena) -> (Vec<multitude::Arc<u64>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(black_box(arena.alloc_arc(black_box(i as u64))));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_arc_with(arena: Arena) -> (Vec<multitude::Arc<u64>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for i in 0..N {
        h.push(black_box(arena.alloc_arc_with(|| black_box(i as u64))));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_uninit_arc(arena: Arena) -> (Vec<multitude::Arc<core::mem::MaybeUninit<u64>>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(black_box(arena.alloc_uninit_arc::<u64>()));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_zeroed_arc(arena: Arena) -> (Vec<multitude::Arc<core::mem::MaybeUninit<u64>>>, Arena) {
    let mut h = Vec::with_capacity(N);
    for _ in 0..N {
        h.push(black_box(arena.alloc_zeroed_arc::<u64>()));
    }
    (h, arena)
}

#[library_benchmark]
#[bench::run(setup_bumpalo())]
fn alloc_u64_bumpalo(bump: bumpalo::Bump) -> bumpalo::Bump {
    for i in 0..N {
        let _: &mut u64 = black_box(black_box(&bump).alloc(black_box(i as u64)));
    }
    bump
}

#[library_benchmark]
#[bench::run(setup_bumpalo())]
fn alloc_u64_bumpalo_with(bump: bumpalo::Bump) -> bumpalo::Bump {
    for i in 0..N {
        let _: &mut u64 = black_box(black_box(&bump).alloc_with(|| black_box(i as u64)));
    }
    bump
}

// ===== alloc_str: single &str allocation =====

#[library_benchmark]
#[bench::run(setup_multitude(), setup_word_inputs())]
fn alloc_str(arena: Arena, words: Vec<String>) -> (Vec<*mut str>, Arena) {
    let mut out: Vec<*mut str> = Vec::with_capacity(N);
    for w in &words {
        let s: &mut str = black_box(arena.alloc_str(black_box(w)));
        out.push(s as *mut str);
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_word_inputs())]
fn alloc_str_box(arena: Arena, words: Vec<String>) -> (Vec<multitude::strings::BoxStr>, Arena) {
    let mut out = Vec::with_capacity(N);
    for w in &words {
        out.push(black_box(arena.alloc_str_box(black_box(w))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_word_inputs())]
fn alloc_str_rc(arena: Arena, words: Vec<String>) -> (Vec<multitude::strings::RcStr>, Arena) {
    let mut out = Vec::with_capacity(N);
    for w in &words {
        out.push(black_box(arena.alloc_str_rc(black_box(w))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_word_inputs())]
fn alloc_str_arc(arena: Arena, words: Vec<String>) -> (Vec<multitude::strings::ArcStr>, Arena) {
    let mut out = Vec::with_capacity(N);
    for w in &words {
        out.push(black_box(arena.alloc_str_arc(black_box(w))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_word_inputs())]
fn alloc_str_bumpalo(bump: bumpalo::Bump, words: Vec<String>) -> (Vec<*mut str>, bumpalo::Bump) {
    let mut out: Vec<*mut str> = Vec::with_capacity(N);
    for w in &words {
        let s: &mut str = black_box(black_box(&bump).alloc_str(black_box(w)));
        out.push(s as *mut str);
    }
    (out, bump)
}

// ===== alloc_slice: slice<u64>, len = SLICE_LEN, N batches =====

#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_copy(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> Arena {
    for s in &slices {
        let _: &mut [u64] = black_box(arena.alloc_slice_copy(black_box(s)));
    }
    arena
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_clone(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> Arena {
    for s in &slices {
        let _: &mut [u64] = black_box(arena.alloc_slice_clone(black_box(s.as_slice())));
    }
    arena
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_with(arena: Arena) -> Arena {
    for _ in 0..N {
        let _: &mut [u64] = black_box(arena.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
    }
    arena
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_iter(arena: Arena) -> Arena {
    for _ in 0..N {
        let _: &mut [u64] = black_box(arena.alloc_slice_fill_iter((0..SLICE_LEN).map(|j| black_box(j as u64))));
    }
    arena
}

// box variants
#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_copy_box(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> (Vec<multitude::Box<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for s in &slices {
        out.push(black_box(arena.alloc_slice_copy_box(black_box(s))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_clone_box(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> (Vec<multitude::Box<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for s in &slices {
        out.push(black_box(arena.alloc_slice_clone_box(black_box(s.as_slice()))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_with_box(arena: Arena) -> (Vec<multitude::Box<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_with_box::<u64, _>(SLICE_LEN, |j| black_box(j as u64)),
        ));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_iter_box(arena: Arena) -> (Vec<multitude::Box<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_iter_box((0..SLICE_LEN).map(|j| black_box(j as u64))),
        ));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_uninit_slice_box(arena: Arena) -> (Vec<multitude::Box<[core::mem::MaybeUninit<u64>]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_slice_box::<u64>(SLICE_LEN)));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_zeroed_slice_box(arena: Arena) -> (Vec<multitude::Box<[core::mem::MaybeUninit<u64>]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_slice_box::<u64>(SLICE_LEN)));
    }
    (out, arena)
}

// rc variants
#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_copy_rc(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> (Vec<multitude::Rc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for s in &slices {
        out.push(black_box(arena.alloc_slice_copy_rc(black_box(s))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_clone_rc(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> (Vec<multitude::Rc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for s in &slices {
        out.push(black_box(arena.alloc_slice_clone_rc(black_box(s.as_slice()))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_with_rc(arena: Arena) -> (Vec<multitude::Rc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_with_rc::<u64, _>(SLICE_LEN, |j| black_box(j as u64)),
        ));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_iter_rc(arena: Arena) -> (Vec<multitude::Rc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_iter_rc((0..SLICE_LEN).map(|j| black_box(j as u64))),
        ));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_uninit_slice_rc(arena: Arena) -> (Vec<multitude::Rc<[core::mem::MaybeUninit<u64>]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_slice_rc::<u64>(SLICE_LEN)));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_zeroed_slice_rc(arena: Arena) -> (Vec<multitude::Rc<[core::mem::MaybeUninit<u64>]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_slice_rc::<u64>(SLICE_LEN)));
    }
    (out, arena)
}

// arc variants
#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_copy_arc(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> (Vec<multitude::Arc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for s in &slices {
        out.push(black_box(arena.alloc_slice_copy_arc(black_box(s))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_slice_inputs())]
fn alloc_slice_clone_arc(arena: Arena, slices: Vec<[u64; SLICE_LEN]>) -> (Vec<multitude::Arc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for s in &slices {
        out.push(black_box(arena.alloc_slice_clone_arc(black_box(s.as_slice()))));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_with_arc(arena: Arena) -> (Vec<multitude::Arc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_with_arc::<u64, _>(SLICE_LEN, |j| black_box(j as u64)),
        ));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_slice_fill_iter_arc(arena: Arena) -> (Vec<multitude::Arc<[u64]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(
            arena.alloc_slice_fill_iter_arc((0..SLICE_LEN).map(|j| black_box(j as u64))),
        ));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_uninit_slice_arc(arena: Arena) -> (Vec<multitude::Arc<[core::mem::MaybeUninit<u64>]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(arena.alloc_uninit_slice_arc::<u64>(SLICE_LEN)));
    }
    (out, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude())]
fn alloc_zeroed_slice_arc(arena: Arena) -> (Vec<multitude::Arc<[core::mem::MaybeUninit<u64>]>>, Arena) {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        out.push(black_box(arena.alloc_zeroed_slice_arc::<u64>(SLICE_LEN)));
    }
    (out, arena)
}

// bumpalo slice variants
#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_slice_inputs())]
fn alloc_slice_bumpalo_copy(bump: bumpalo::Bump, slices: Vec<[u64; SLICE_LEN]>) -> bumpalo::Bump {
    for s in &slices {
        let _: &mut [u64] = black_box(black_box(&bump).alloc_slice_copy(black_box(s.as_slice())));
    }
    bump
}

#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_slice_inputs())]
fn alloc_slice_bumpalo_clone(bump: bumpalo::Bump, slices: Vec<[u64; SLICE_LEN]>) -> bumpalo::Bump {
    for s in &slices {
        let _: &mut [u64] = black_box(black_box(&bump).alloc_slice_clone(black_box(s.as_slice())));
    }
    bump
}

#[library_benchmark]
#[bench::run(setup_bumpalo())]
fn alloc_slice_bumpalo_fill_with(bump: bumpalo::Bump) -> bumpalo::Bump {
    for _ in 0..N {
        let _: &mut [u64] = black_box(black_box(&bump).alloc_slice_fill_with::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
    }
    bump
}

#[library_benchmark]
#[bench::run(setup_bumpalo())]
fn alloc_slice_bumpalo_fill_iter(bump: bumpalo::Bump) -> bumpalo::Bump {
    for _ in 0..N {
        let _: &mut [u64] = black_box(black_box(&bump).alloc_slice_fill_iter((0..SLICE_LEN).map(|j| black_box(j as u64))));
    }
    bump
}

// ===== string_builder: push N tokens, freeze =====

#[library_benchmark]
#[bench::run(setup_multitude(), setup_word_inputs())]
fn alloc_string(arena: Arena, words: Vec<String>) -> (multitude::strings::RcStr, Arena) {
    let mut s = arena.alloc_string();
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen = black_box(s.into_arena_str());
    (frozen, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_word_inputs())]
fn alloc_string_with_capacity(arena: Arena, words: Vec<String>) -> (multitude::strings::RcStr, Arena) {
    let mut s = arena.alloc_string_with_capacity(N * 6);
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen = black_box(s.into_arena_str());
    (frozen, arena)
}

#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_word_inputs())]
fn string_builder_bumpalo_grow(bump: bumpalo::Bump, words: Vec<String>) -> (*const str, bumpalo::Bump) {
    let mut s = bumpalo::collections::String::new_in(&bump);
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen: &str = black_box(s.into_bump_str());
    (frozen as *const str, bump)
}

#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_word_inputs())]
fn string_builder_bumpalo_with_cap(bump: bumpalo::Bump, words: Vec<String>) -> (*const str, bumpalo::Bump) {
    let mut s = bumpalo::collections::String::with_capacity_in(N * 6, &bump);
    for w in &words {
        s.push_str(black_box(w.as_str()));
    }
    let frozen: &str = black_box(s.into_bump_str());
    (frozen as *const str, bump)
}

// ===== vec_builder: push N i32, freeze =====

#[library_benchmark]
#[bench::run(setup_multitude(), setup_int_inputs())]
fn alloc_vec(arena: Arena, ints: Vec<i32>) -> (multitude::Rc<[i32]>, Arena) {
    let mut v = arena.alloc_vec::<i32>();
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen = black_box(v.into_arena_rc());
    (frozen, arena)
}

#[library_benchmark]
#[bench::run(setup_multitude(), setup_int_inputs())]
fn alloc_vec_with_capacity(arena: Arena, ints: Vec<i32>) -> (multitude::Rc<[i32]>, Arena) {
    let mut v = arena.alloc_vec_with_capacity::<i32>(N);
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen = black_box(v.into_arena_rc());
    (frozen, arena)
}

#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_int_inputs())]
fn vec_builder_bumpalo_grow(bump: bumpalo::Bump, ints: Vec<i32>) -> (*const [i32], bumpalo::Bump) {
    let mut v: bumpalo::collections::Vec<'_, i32> = bumpalo::collections::Vec::new_in(&bump);
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen: &[i32] = black_box(v.into_bump_slice());
    (frozen as *const [i32], bump)
}

#[library_benchmark]
#[bench::run(setup_bumpalo(), setup_int_inputs())]
fn vec_builder_bumpalo_with_cap(bump: bumpalo::Bump, ints: Vec<i32>) -> (*const [i32], bumpalo::Bump) {
    let mut v: bumpalo::collections::Vec<'_, i32> = bumpalo::collections::Vec::with_capacity_in(N, &bump);
    for &i in &ints {
        v.push(black_box(i));
    }
    let frozen: &[i32] = black_box(v.into_bump_slice());
    (frozen as *const [i32], bump)
}

// ===== arena_creation: standalone Arena/Bump construction + drop =====

#[library_benchmark]
fn arena_creation_multitude() {
    let arena = black_box(Arena::new());
    drop(arena);
}

#[library_benchmark]
fn arena_creation_bumpalo() {
    let bump = black_box(bumpalo::Bump::new());
    drop(bump);
}

library_benchmark_group!(
    name = alloc_group;
    benchmarks =
        arena_creation_multitude, arena_creation_bumpalo,
        alloc, alloc_with,
        alloc_box, alloc_box_with,
        alloc_uninit_box, alloc_zeroed_box,
        alloc_rc, alloc_rc_with,
        alloc_uninit_rc, alloc_zeroed_rc,
        alloc_arc, alloc_arc_with,
        alloc_uninit_arc, alloc_zeroed_arc,
        alloc_u64_bumpalo, alloc_u64_bumpalo_with,
        alloc_str, alloc_str_box,
        alloc_str_rc, alloc_str_arc, alloc_str_bumpalo,
        alloc_slice_copy, alloc_slice_clone,
        alloc_slice_fill_with, alloc_slice_fill_iter,
        alloc_slice_copy_box, alloc_slice_clone_box,
        alloc_slice_fill_with_box, alloc_slice_fill_iter_box,
        alloc_uninit_slice_box, alloc_zeroed_slice_box,
        alloc_slice_copy_rc, alloc_slice_clone_rc,
        alloc_slice_fill_with_rc, alloc_slice_fill_iter_rc,
        alloc_uninit_slice_rc, alloc_zeroed_slice_rc,
        alloc_slice_copy_arc, alloc_slice_clone_arc,
        alloc_slice_fill_with_arc, alloc_slice_fill_iter_arc,
        alloc_uninit_slice_arc, alloc_zeroed_slice_arc,
        alloc_slice_bumpalo_copy, alloc_slice_bumpalo_clone,
        alloc_slice_bumpalo_fill_with, alloc_slice_bumpalo_fill_iter,
        alloc_string, alloc_string_with_capacity,
        string_builder_bumpalo_grow, string_builder_bumpalo_with_cap,
        alloc_vec, alloc_vec_with_capacity,
        vec_builder_bumpalo_grow, vec_builder_bumpalo_with_cap
);
