// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Linux Callgrind wrappers for the shared allocation benchmark hot paths.

#![allow(missing_docs, reason = "benchmark code")]
#![allow(clippy::needless_pass_by_value, reason = "Gungraun setup state")]
#![allow(clippy::too_many_lines, reason = "benchmark registrations")]
#![allow(clippy::type_complexity, reason = "benchmark state tuples")]

#[path = "../multitude_alloc_common/mod.rs"]
mod alloc_common;

use core::mem::MaybeUninit;

use alloc_common as common;
use gungraun::{library_benchmark, library_benchmark_group};
use multitude::{Arc, Arena, Box, Rc};

#[library_benchmark]
fn multitude_new() {
    common::multitude_new();
}

#[library_benchmark]
fn bumpalo_new() {
    common::bumpalo_new();
}

macro_rules! arena_only {
    ($name:ident, $setup:expr, $hot:path) => {
        #[library_benchmark]
        #[bench::run($setup)]
        fn $name(arena: Arena) -> Arena {
            $hot(&arena);
            arena
        }
    };
}

arena_only!(alloc, common::warm_arena_local(), common::alloc);
arena_only!(alloc_with, common::warm_arena_local(), common::alloc_with);
arena_only!(alloc_slice_fill_with, common::warm_arena_local(), common::alloc_slice_fill_with);
arena_only!(alloc_slice_fill_iter, common::warm_arena_local(), common::alloc_slice_fill_iter);
arena_only!(allocator_grow_in_place, common::warm_arena_local(), common::allocator_grow_in_place);
arena_only!(
    allocator_grow_zeroed_in_place,
    common::warm_arena_local(),
    common::allocator_grow_zeroed_in_place
);
arena_only!(
    allocator_shrink_in_place,
    common::warm_arena_local(),
    common::allocator_shrink_in_place
);

macro_rules! bump_only {
    ($name:ident, $hot:path) => {
        #[library_benchmark]
        #[bench::run(common::warm_bump())]
        fn $name(bump: bumpalo::Bump) -> bumpalo::Bump {
            $hot(&bump);
            bump
        }
    };
}

bump_only!(bumpalo_alloc, common::bumpalo_alloc);
bump_only!(bumpalo_alloc_with, common::bumpalo_alloc_with);
bump_only!(bumpalo_alloc_slice_fill_with, common::bumpalo_alloc_slice_fill_with);
bump_only!(bumpalo_alloc_slice_fill_iter, common::bumpalo_alloc_slice_fill_iter);

macro_rules! arena_collect {
    ($name:ident, $ty:ty, $count:expr, $hot:path) => {
        #[library_benchmark]
        #[bench::run(common::setup_arena_out($count))]
        fn $name(state: (Arena, Vec<$ty>)) -> (Arena, Vec<$ty>) {
            let (arena, mut out) = state;
            $hot(&arena, &mut out);
            (arena, out)
        }
    };
}

arena_collect!(alloc_box, Box<u64>, common::N, common::alloc_box);
arena_collect!(alloc_box_with, Box<u64>, common::N, common::alloc_box_with);
arena_collect!(alloc_uninit_box, Box<MaybeUninit<u64>>, common::N, common::alloc_uninit_box);
arena_collect!(alloc_zeroed_box, Box<MaybeUninit<u64>>, common::N, common::alloc_zeroed_box);
arena_collect!(alloc_arc, Arc<u64>, common::N, common::alloc_arc);
arena_collect!(alloc_arc_with, Arc<u64>, common::N, common::alloc_arc_with);
arena_collect!(alloc_uninit_arc, Arc<MaybeUninit<u64>>, common::N, common::alloc_uninit_arc);
arena_collect!(alloc_zeroed_arc, Arc<MaybeUninit<u64>>, common::N, common::alloc_zeroed_arc);
arena_collect!(alloc_rc, Rc<u64>, common::N, common::alloc_rc);
arena_collect!(alloc_rc_with, Rc<u64>, common::N, common::alloc_rc_with);
arena_collect!(alloc_uninit_rc, Rc<MaybeUninit<u64>>, common::N, common::alloc_uninit_rc);
arena_collect!(alloc_zeroed_rc, Rc<MaybeUninit<u64>>, common::N, common::alloc_zeroed_rc);

#[library_benchmark]
#[bench::run(common::setup_arena_words())]
fn alloc_str(state: (Arena, Vec<String>)) -> (Arena, Vec<String>) {
    let (arena, words) = state;
    common::alloc_str(&arena, &words);
    (arena, words)
}

macro_rules! arena_words_collect {
    ($name:ident, $ty:ty, $hot:path) => {
        #[library_benchmark]
        #[bench::run(common::setup_arena_words_out())]
        fn $name(state: (Arena, Vec<String>, Vec<$ty>)) -> (Arena, Vec<String>, Vec<$ty>) {
            let (arena, words, mut out) = state;
            $hot(&arena, &words, &mut out);
            (arena, words, out)
        }
    };
}

arena_words_collect!(alloc_str_box, Box<str>, common::alloc_str_box);
arena_words_collect!(alloc_str_arc, Arc<str>, common::alloc_str_arc);
arena_words_collect!(alloc_str_rc, Rc<str>, common::alloc_str_rc);

#[library_benchmark]
#[bench::run(common::setup_bump_words())]
fn bumpalo_alloc_str(state: (bumpalo::Bump, Vec<String>)) -> (bumpalo::Bump, Vec<String>) {
    let (bump, words) = state;
    common::bumpalo_alloc_str(&bump, &words);
    (bump, words)
}

macro_rules! arena_slice_input {
    ($name:ident, $count:expr, $hot:path) => {
        #[library_benchmark]
        #[bench::run(common::setup_arena_slices($count))]
        fn $name(state: (Arena, Vec<[u64; common::SLICE_LEN]>)) -> (Arena, Vec<[u64; common::SLICE_LEN]>) {
            let (arena, slices) = state;
            $hot(&arena, &slices);
            (arena, slices)
        }
    };
}

arena_slice_input!(alloc_slice_copy, common::N, common::alloc_slice_copy);
arena_slice_input!(alloc_slice_clone, common::N, common::alloc_slice_clone);

macro_rules! arena_slice_collect {
    ($name:ident, $ty:ty, $count:expr, $hot:path) => {
        #[library_benchmark]
        #[bench::run(common::setup_arena_slices_out($count))]
        fn $name(state: (Arena, Vec<[u64; common::SLICE_LEN]>, Vec<$ty>)) -> (Arena, Vec<[u64; common::SLICE_LEN]>, Vec<$ty>) {
            let (arena, slices, mut out) = state;
            $hot(&arena, &slices, &mut out);
            (arena, slices, out)
        }
    };
}

arena_slice_collect!(
    alloc_slice_copy_box,
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_copy_box
);
arena_slice_collect!(
    alloc_slice_clone_box,
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_clone_box
);
arena_collect!(
    alloc_slice_fill_with_box,
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_with_box
);
arena_collect!(
    alloc_slice_fill_iter_box,
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_iter_box
);
arena_collect!(
    alloc_uninit_slice_box,
    Box<[MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_uninit_slice_box
);
arena_collect!(
    alloc_zeroed_slice_box,
    Box<[MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_zeroed_slice_box
);

arena_slice_collect!(
    alloc_slice_copy_arc,
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_copy_arc
);
arena_slice_collect!(
    alloc_slice_clone_arc,
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_clone_arc
);
arena_collect!(
    alloc_slice_fill_with_arc,
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_with_arc
);
arena_collect!(
    alloc_slice_fill_iter_arc,
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_iter_arc
);
arena_collect!(
    alloc_uninit_slice_arc,
    Arc<[MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_uninit_slice_arc
);
arena_collect!(
    alloc_zeroed_slice_arc,
    Arc<[MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_zeroed_slice_arc
);

arena_slice_collect!(alloc_slice_copy_rc, Rc<[u64]>, common::OWNED_SLICE_N, common::alloc_slice_copy_rc);
arena_slice_collect!(alloc_slice_clone_rc, Rc<[u64]>, common::OWNED_SLICE_N, common::alloc_slice_clone_rc);
arena_collect!(
    alloc_slice_fill_with_rc,
    Rc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_with_rc
);
arena_collect!(
    alloc_slice_fill_iter_rc,
    Rc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_iter_rc
);
arena_collect!(
    alloc_uninit_slice_rc,
    Rc<[MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_uninit_slice_rc
);
arena_collect!(
    alloc_zeroed_slice_rc,
    Rc<[MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_zeroed_slice_rc
);

#[library_benchmark]
#[bench::run(common::setup_bump_slices())]
fn bumpalo_alloc_slice_copy(state: (bumpalo::Bump, Vec<[u64; common::SLICE_LEN]>)) -> (bumpalo::Bump, Vec<[u64; common::SLICE_LEN]>) {
    let (bump, slices) = state;
    common::bumpalo_alloc_slice_copy(&bump, &slices);
    (bump, slices)
}

#[library_benchmark]
#[bench::run(common::setup_bump_slices())]
fn bumpalo_alloc_slice_clone(state: (bumpalo::Bump, Vec<[u64; common::SLICE_LEN]>)) -> (bumpalo::Bump, Vec<[u64; common::SLICE_LEN]>) {
    let (bump, slices) = state;
    common::bumpalo_alloc_slice_clone(&bump, &slices);
    (bump, slices)
}

#[library_benchmark]
#[bench::run(common::setup_arena_words())]
fn alloc_string(state: (Arena, Vec<String>)) -> (*const str, Arena, Vec<String>) {
    let (arena, words) = state;
    let pointer = common::alloc_string(&arena, &words);
    (pointer, arena, words)
}

#[library_benchmark]
#[bench::run(common::setup_arena_words_with_len())]
fn alloc_string_with_capacity(state: (Arena, Vec<String>, usize)) -> (*const str, Arena, Vec<String>) {
    let (arena, words, len) = state;
    let pointer = common::alloc_string_with_capacity(&arena, &words, len);
    (pointer, arena, words)
}

#[library_benchmark]
#[bench::run(common::setup_bump_words())]
fn bumpalo_string_new_in(state: (bumpalo::Bump, Vec<String>)) -> (*const str, bumpalo::Bump, Vec<String>) {
    let (bump, words) = state;
    let pointer = common::bumpalo_string_new_in(&bump, &words);
    (pointer, bump, words)
}

#[library_benchmark]
#[bench::run(common::setup_bump_words_with_len())]
fn bumpalo_string_with_capacity_in(state: (bumpalo::Bump, Vec<String>, usize)) -> (*const str, bumpalo::Bump, Vec<String>) {
    let (bump, words, len) = state;
    let pointer = common::bumpalo_string_with_capacity_in(&bump, &words, len);
    (pointer, bump, words)
}

#[library_benchmark]
#[bench::run(common::setup_arena_ints())]
fn alloc_vec(state: (Arena, Vec<i32>)) -> (*const [i32], Arena, Vec<i32>) {
    let (arena, ints) = state;
    let pointer = common::alloc_vec(&arena, &ints);
    (pointer, arena, ints)
}

#[library_benchmark]
#[bench::run(common::setup_arena_ints())]
fn alloc_vec_with_capacity(state: (Arena, Vec<i32>)) -> (*const [i32], Arena, Vec<i32>) {
    let (arena, ints) = state;
    let pointer = common::alloc_vec_with_capacity(&arena, &ints);
    (pointer, arena, ints)
}

#[library_benchmark]
#[bench::run(common::setup_bump_ints())]
fn bumpalo_vec_new_in(state: (bumpalo::Bump, Vec<i32>)) -> (*const [i32], bumpalo::Bump, Vec<i32>) {
    let (bump, ints) = state;
    let pointer = common::bumpalo_vec_new_in(&bump, &ints);
    (pointer, bump, ints)
}

#[library_benchmark]
#[bench::run(common::setup_bump_ints())]
fn bumpalo_vec_with_capacity_in(state: (bumpalo::Bump, Vec<i32>)) -> (*const [i32], bumpalo::Bump, Vec<i32>) {
    let (bump, ints) = state;
    let pointer = common::bumpalo_vec_with_capacity_in(&bump, &ints);
    (pointer, bump, ints)
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
        alloc_rc, alloc_rc_with,
        alloc_uninit_rc, alloc_zeroed_rc,
        bumpalo_alloc, bumpalo_alloc_with,
        alloc_str, alloc_str_box,
        alloc_str_arc, alloc_str_rc, bumpalo_alloc_str,
        alloc_slice_copy, alloc_slice_clone,
        alloc_slice_fill_with, alloc_slice_fill_iter,
        alloc_slice_copy_box, alloc_slice_clone_box,
        alloc_slice_fill_with_box, alloc_slice_fill_iter_box,
        alloc_uninit_slice_box, alloc_zeroed_slice_box,
        alloc_slice_copy_arc, alloc_slice_clone_arc,
        alloc_slice_fill_with_arc, alloc_slice_fill_iter_arc,
        alloc_uninit_slice_arc, alloc_zeroed_slice_arc,
        alloc_slice_copy_rc, alloc_slice_clone_rc,
        alloc_slice_fill_with_rc, alloc_slice_fill_iter_rc,
        alloc_uninit_slice_rc, alloc_zeroed_slice_rc,
        bumpalo_alloc_slice_copy, bumpalo_alloc_slice_clone,
        bumpalo_alloc_slice_fill_with, bumpalo_alloc_slice_fill_iter,
        alloc_string, alloc_string_with_capacity,
        bumpalo_string_new_in, bumpalo_string_with_capacity_in,
        alloc_vec, alloc_vec_with_capacity,
        bumpalo_vec_new_in, bumpalo_vec_with_capacity_in
);

library_benchmark_group!(
    name = allocator_grow_group;
    benchmarks =
        allocator_grow_in_place,
        allocator_grow_zeroed_in_place,
        allocator_shrink_in_place
);
