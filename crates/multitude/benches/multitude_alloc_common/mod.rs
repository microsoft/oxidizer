// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared setup and measured functions for the allocation benchmarks.

#![allow(missing_docs, reason = "benchmark support module")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(unreachable_pub, reason = "shared between separate benchmark crates")]

use core::alloc::Layout;
use core::hint::black_box;
use core::mem::MaybeUninit;
use core::ptr;

use allocator_api2::alloc::Allocator;
use multitude::{Arc, Arena, Box, Rc};

pub const N: usize = 1_000;
// Arc/Rc slices reserve at most 80 bytes (16-byte prefix + 64-byte payload).
// This count leaves room for the chunk header and setup prime in one 64 KiB
// normal chunk; Box slices are capped identically for direct comparison.
pub const OWNED_SLICE_N: usize = 768;
pub const SLICE_LEN: usize = 8;

const PREALLOC_BYTES: usize = 64 * 1024;
// Touch enough storage for the 64,000-byte direct slice workload after its
// prime, while keeping every reservation on the normal-allocation path.
const PAGE_TOUCH_BYTES: usize = 64_256;
const PAGE_TOUCH_BLOCK_BYTES: usize = 4 * 1024;
const SMALL_LAYOUT: Layout = Layout::new::<u64>();
// SAFETY: 32 is nonzero and 8 is a power of two that divides 32.
const LARGE_LAYOUT: Layout = unsafe { Layout::from_size_align_unchecked(32, 8) };

fn touch_arena_pages(arena: &Arena) {
    let mut remaining = PAGE_TOUCH_BYTES;
    while remaining != 0 {
        let len = remaining.min(PAGE_TOUCH_BLOCK_BYTES);
        let _ = black_box(arena.alloc_slice_fill_with(len, |_| 0_u8));
        remaining -= len;
    }
}

fn touch_bump_pages(bump: &bumpalo::Bump) {
    let mut remaining = PAGE_TOUCH_BYTES;
    while remaining != 0 {
        let len = remaining.min(PAGE_TOUCH_BLOCK_BYTES);
        let _ = black_box(bump.alloc_slice_fill_copy(len, 0_u8));
        remaining -= len;
    }
}

pub fn warm_arena_local() -> Arena {
    let mut arena = Arena::builder().with_capacity(PREALLOC_BYTES).build();
    touch_arena_pages(&arena);
    arena.reset();
    let _ = arena.alloc(0_u64);
    arena
}

pub fn warm_arena_shared() -> Arena {
    let mut arena = Arena::builder().with_capacity(PREALLOC_BYTES).build();
    touch_arena_pages(&arena);
    arena.reset();
    let _ = arena.alloc_arc(0_u64);
    arena
}

pub fn warm_bump() -> bumpalo::Bump {
    let mut bump = bumpalo::Bump::with_capacity(PREALLOC_BYTES);
    touch_bump_pages(&bump);
    bump.reset();
    let _ = bump.alloc(0_u64);
    bump
}

pub fn word_inputs() -> Vec<String> {
    (0..N).map(|i| format!("word{i}")).collect()
}

pub fn int_inputs() -> Vec<i32> {
    (0..N).map(|i| i32::try_from(i).unwrap_or(0)).collect()
}

pub fn slice_inputs(count: usize) -> Vec<[u64; SLICE_LEN]> {
    (0..count)
        .map(|i| {
            let base = i as u64;
            [base, base + 1, base + 2, base + 3, base + 4, base + 5, base + 6, base + 7]
        })
        .collect()
}

pub fn setup_arena_out<T>(count: usize) -> (Arena, Vec<T>) {
    let out = Vec::with_capacity(count);
    let arena = warm_arena_shared();
    (arena, out)
}

pub fn setup_arena_words() -> (Arena, Vec<String>) {
    let words = word_inputs();
    let arena = warm_arena_local();
    (arena, words)
}

pub fn setup_arena_words_with_len() -> (Arena, Vec<String>, usize) {
    let words = word_inputs();
    let len = words.iter().map(String::len).sum();
    let arena = warm_arena_local();
    (arena, words, len)
}

pub fn setup_arena_words_out<T>() -> (Arena, Vec<String>, Vec<T>) {
    let words = word_inputs();
    let out = Vec::with_capacity(N);
    let arena = warm_arena_shared();
    (arena, words, out)
}

pub fn setup_arena_ints() -> (Arena, Vec<i32>) {
    let ints = int_inputs();
    let arena = warm_arena_local();
    (arena, ints)
}

pub fn setup_arena_slices(count: usize) -> (Arena, Vec<[u64; SLICE_LEN]>) {
    let slices = slice_inputs(count);
    let arena = warm_arena_local();
    (arena, slices)
}

pub fn setup_arena_slices_out<T>(count: usize) -> (Arena, Vec<[u64; SLICE_LEN]>, Vec<T>) {
    let slices = slice_inputs(count);
    let out = Vec::with_capacity(count);
    let arena = warm_arena_shared();
    (arena, slices, out)
}

pub fn setup_bump_words() -> (bumpalo::Bump, Vec<String>) {
    let words = word_inputs();
    let bump = warm_bump();
    (bump, words)
}

pub fn setup_bump_words_with_len() -> (bumpalo::Bump, Vec<String>, usize) {
    let words = word_inputs();
    let len = words.iter().map(String::len).sum();
    let bump = warm_bump();
    (bump, words, len)
}

pub fn setup_bump_ints() -> (bumpalo::Bump, Vec<i32>) {
    let ints = int_inputs();
    let bump = warm_bump();
    (bump, ints)
}

pub fn setup_bump_slices() -> (bumpalo::Bump, Vec<[u64; SLICE_LEN]>) {
    let slices = slice_inputs(N);
    let bump = warm_bump();
    (bump, slices)
}

#[inline(never)]
pub fn multitude_new() {
    let arena = Arena::new();
    black_box(&arena);
    drop(arena);
}

#[inline(never)]
pub fn bumpalo_new() {
    let bump = bumpalo::Bump::new();
    black_box(&bump);
    drop(bump);
}

#[inline(never)]
pub fn alloc(arena: &Arena) {
    for i in 0..N {
        let _ = black_box(arena.alloc(black_box(i as u64)));
    }
}

#[inline(never)]
pub fn alloc_with(arena: &Arena) {
    for i in 0..N {
        let _ = black_box(arena.alloc_with(|| black_box(i as u64)));
    }
}

#[inline(never)]
pub fn bumpalo_alloc(bump: &bumpalo::Bump) {
    for i in 0..N {
        let _ = black_box(bump.alloc(black_box(i as u64)));
    }
}

#[inline(never)]
pub fn bumpalo_alloc_with(bump: &bumpalo::Bump) {
    for i in 0..N {
        let _ = black_box(bump.alloc_with(|| black_box(i as u64)));
    }
}

macro_rules! scalar_collect {
    ($name:ident, $ty:ty, $count:expr, $allocate:expr) => {
        #[inline(never)]
        pub fn $name(arena: &Arena, out: &mut Vec<$ty>) {
            for i in 0..$count {
                out.push(($allocate)(arena, i));
            }
        }
    };
}

scalar_collect!(alloc_box, Box<u64>, N, |arena: &Arena, i| arena.alloc_box(black_box(i as u64)));
scalar_collect!(alloc_box_with, Box<u64>, N, |arena: &Arena, i| arena
    .alloc_box_with(|| black_box(i as u64)));
scalar_collect!(alloc_uninit_box, Box<MaybeUninit<u64>>, N, |arena: &Arena, _| arena
    .alloc_uninit_box::<u64>());
scalar_collect!(alloc_zeroed_box, Box<MaybeUninit<u64>>, N, |arena: &Arena, _| arena
    .alloc_zeroed_box::<u64>());
scalar_collect!(alloc_arc, Arc<u64>, N, |arena: &Arena, i| arena.alloc_arc(black_box(i as u64)));
scalar_collect!(alloc_arc_with, Arc<u64>, N, |arena: &Arena, i| arena
    .alloc_arc_with(|| black_box(i as u64)));
scalar_collect!(alloc_uninit_arc, Arc<MaybeUninit<u64>>, N, |arena: &Arena, _| arena
    .alloc_uninit_arc::<u64>());
scalar_collect!(alloc_zeroed_arc, Arc<MaybeUninit<u64>>, N, |arena: &Arena, _| arena
    .alloc_zeroed_arc::<u64>());
scalar_collect!(alloc_rc, Rc<u64>, N, |arena: &Arena, i| arena.alloc_rc(black_box(i as u64)));
scalar_collect!(alloc_rc_with, Rc<u64>, N, |arena: &Arena, i| arena
    .alloc_rc_with(|| black_box(i as u64)));
scalar_collect!(alloc_uninit_rc, Rc<MaybeUninit<u64>>, N, |arena: &Arena, _| arena
    .alloc_uninit_rc::<u64>());
scalar_collect!(alloc_zeroed_rc, Rc<MaybeUninit<u64>>, N, |arena: &Arena, _| arena
    .alloc_zeroed_rc::<u64>());

#[inline(never)]
pub fn alloc_str(arena: &Arena, words: &[String]) {
    for word in words {
        let _ = black_box(arena.alloc_str(black_box(word.as_str())));
    }
}

#[inline(never)]
pub fn bumpalo_alloc_str(bump: &bumpalo::Bump, words: &[String]) {
    for word in words {
        let _ = black_box(bump.alloc_str(black_box(word.as_str())));
    }
}

macro_rules! str_collect {
    ($name:ident, $ty:ty, $allocate:ident) => {
        #[inline(never)]
        pub fn $name(arena: &Arena, words: &[String], out: &mut Vec<$ty>) {
            for word in words {
                out.push(arena.$allocate(black_box(word.as_str())));
            }
        }
    };
}

str_collect!(alloc_str_box, Box<str>, alloc_str_box);
str_collect!(alloc_str_arc, Arc<str>, alloc_str_arc);
str_collect!(alloc_str_rc, Rc<str>, alloc_str_rc);

#[inline(never)]
pub fn alloc_slice_copy(arena: &Arena, slices: &[[u64; SLICE_LEN]]) {
    for slice in slices {
        let _ = black_box(arena.alloc_slice_copy(black_box(slice.as_slice())));
    }
}

#[inline(never)]
pub fn alloc_slice_clone(arena: &Arena, slices: &[[u64; SLICE_LEN]]) {
    for slice in slices {
        let _ = black_box(arena.alloc_slice_clone(black_box(slice.as_slice())));
    }
}

#[inline(never)]
pub fn alloc_slice_fill_with(arena: &Arena) {
    for _ in 0..N {
        let _ = black_box(arena.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |i| i as u64));
    }
}

#[inline(never)]
pub fn alloc_slice_fill_iter(arena: &Arena) {
    for _ in 0..N {
        let _ = black_box(arena.alloc_slice_fill_iter((0..SLICE_LEN).map(|i| i as u64)));
    }
}

#[inline(never)]
pub fn bumpalo_alloc_slice_copy(bump: &bumpalo::Bump, slices: &[[u64; SLICE_LEN]]) {
    for slice in slices {
        let _ = black_box(bump.alloc_slice_copy(black_box(slice.as_slice())));
    }
}

#[inline(never)]
pub fn bumpalo_alloc_slice_clone(bump: &bumpalo::Bump, slices: &[[u64; SLICE_LEN]]) {
    for slice in slices {
        let _ = black_box(bump.alloc_slice_clone(black_box(slice.as_slice())));
    }
}

#[inline(never)]
pub fn bumpalo_alloc_slice_fill_with(bump: &bumpalo::Bump) {
    for _ in 0..N {
        let _ = black_box(bump.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |i| i as u64));
    }
}

#[inline(never)]
pub fn bumpalo_alloc_slice_fill_iter(bump: &bumpalo::Bump) {
    for _ in 0..N {
        let _ = black_box(bump.alloc_slice_fill_iter((0..SLICE_LEN).map(|i| i as u64)));
    }
}

macro_rules! slice_copy_collect {
    ($name:ident, $ty:ty, $allocate:ident) => {
        #[inline(never)]
        pub fn $name(arena: &Arena, slices: &[[u64; SLICE_LEN]], out: &mut Vec<$ty>) {
            for slice in slices {
                out.push(arena.$allocate(black_box(slice.as_slice())));
            }
        }
    };
}

macro_rules! slice_generated_collect {
    ($name:ident, $ty:ty, $count:expr, $allocate:expr) => {
        #[inline(never)]
        pub fn $name(arena: &Arena, out: &mut Vec<$ty>) {
            for _ in 0..$count {
                out.push(($allocate)(arena));
            }
        }
    };
}

slice_copy_collect!(alloc_slice_copy_box, Box<[u64]>, alloc_slice_copy_box);
slice_copy_collect!(alloc_slice_clone_box, Box<[u64]>, alloc_slice_clone_box);
slice_generated_collect!(alloc_slice_fill_with_box, Box<[u64]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_slice_fill_with_box::<u64, _>(SLICE_LEN, |i| i as u64)
});
slice_generated_collect!(alloc_slice_fill_iter_box, Box<[u64]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_slice_fill_iter_box((0..SLICE_LEN).map(|i| i as u64))
});
slice_generated_collect!(alloc_uninit_slice_box, Box<[MaybeUninit<u64>]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_uninit_slice_box::<u64>(SLICE_LEN)
});
slice_generated_collect!(alloc_zeroed_slice_box, Box<[MaybeUninit<u64>]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_zeroed_slice_box::<u64>(SLICE_LEN)
});

slice_copy_collect!(alloc_slice_copy_arc, Arc<[u64]>, alloc_slice_copy_arc);
slice_copy_collect!(alloc_slice_clone_arc, Arc<[u64]>, alloc_slice_clone_arc);
slice_generated_collect!(alloc_slice_fill_with_arc, Arc<[u64]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_slice_fill_with_arc::<u64, _>(SLICE_LEN, |i| i as u64)
});
slice_generated_collect!(alloc_slice_fill_iter_arc, Arc<[u64]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_slice_fill_iter_arc((0..SLICE_LEN).map(|i| i as u64))
});
slice_generated_collect!(alloc_uninit_slice_arc, Arc<[MaybeUninit<u64>]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_uninit_slice_arc::<u64>(SLICE_LEN)
});
slice_generated_collect!(alloc_zeroed_slice_arc, Arc<[MaybeUninit<u64>]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_zeroed_slice_arc::<u64>(SLICE_LEN)
});

slice_copy_collect!(alloc_slice_copy_rc, Rc<[u64]>, alloc_slice_copy_rc);
slice_copy_collect!(alloc_slice_clone_rc, Rc<[u64]>, alloc_slice_clone_rc);
slice_generated_collect!(alloc_slice_fill_with_rc, Rc<[u64]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_slice_fill_with_rc::<u64, _>(SLICE_LEN, |i| i as u64)
});
slice_generated_collect!(alloc_slice_fill_iter_rc, Rc<[u64]>, OWNED_SLICE_N, |arena: &Arena| {
    arena.alloc_slice_fill_iter_rc((0..SLICE_LEN).map(|i| i as u64))
});
slice_generated_collect!(alloc_uninit_slice_rc, Rc<[MaybeUninit<u64>]>, OWNED_SLICE_N, |arena: &Arena| arena
    .alloc_uninit_slice_rc::<u64>(
    SLICE_LEN
));
slice_generated_collect!(alloc_zeroed_slice_rc, Rc<[MaybeUninit<u64>]>, OWNED_SLICE_N, |arena: &Arena| arena
    .alloc_zeroed_slice_rc::<u64>(
    SLICE_LEN
));

#[inline(never)]
pub fn alloc_string(arena: &Arena, words: &[String]) -> *const str {
    let mut string = arena.alloc_string();
    for word in words {
        string.push_str(black_box(word.as_str()));
    }
    black_box(ptr::from_mut(string.leak()).cast_const())
}

#[inline(never)]
pub fn alloc_string_with_capacity(arena: &Arena, words: &[String], capacity: usize) -> *const str {
    let mut string = arena.alloc_string_with_capacity(capacity);
    for word in words {
        string.push_str(black_box(word.as_str()));
    }
    black_box(ptr::from_mut(string.leak()).cast_const())
}

#[inline(never)]
pub fn bumpalo_string_new_in(bump: &bumpalo::Bump, words: &[String]) -> *const str {
    let mut string = bumpalo::collections::String::new_in(bump);
    for word in words {
        string.push_str(black_box(word.as_str()));
    }
    black_box(ptr::from_ref(string.into_bump_str()))
}

#[inline(never)]
pub fn bumpalo_string_with_capacity_in(bump: &bumpalo::Bump, words: &[String], capacity: usize) -> *const str {
    let mut string = bumpalo::collections::String::with_capacity_in(capacity, bump);
    for word in words {
        string.push_str(black_box(word.as_str()));
    }
    black_box(ptr::from_ref(string.into_bump_str()))
}

#[inline(never)]
pub fn alloc_vec(arena: &Arena, ints: &[i32]) -> *const [i32] {
    let mut vec = arena.alloc_vec::<i32>();
    for &value in ints {
        vec.push(black_box(value));
    }
    black_box(ptr::from_mut(vec.leak()).cast_const())
}

#[inline(never)]
pub fn alloc_vec_with_capacity(arena: &Arena, ints: &[i32]) -> *const [i32] {
    let mut vec = arena.alloc_vec_with_capacity::<i32>(N);
    for &value in ints {
        vec.push(black_box(value));
    }
    black_box(ptr::from_mut(vec.leak()).cast_const())
}

#[inline(never)]
pub fn bumpalo_vec_new_in(bump: &bumpalo::Bump, ints: &[i32]) -> *const [i32] {
    let mut vec = bumpalo::collections::Vec::new_in(bump);
    for &value in ints {
        vec.push(black_box(value));
    }
    black_box(ptr::from_ref(vec.into_bump_slice()))
}

#[inline(never)]
pub fn bumpalo_vec_with_capacity_in(bump: &bumpalo::Bump, ints: &[i32]) -> *const [i32] {
    let mut vec = bumpalo::collections::Vec::with_capacity_in(N, bump);
    for &value in ints {
        vec.push(black_box(value));
    }
    black_box(ptr::from_ref(vec.into_bump_slice()))
}

#[inline(never)]
pub fn allocator_grow_in_place(arena: &Arena) {
    for _ in 0..N {
        let ptr = arena.allocate(SMALL_LAYOUT).unwrap().cast::<u8>();
        // SAFETY: `ptr` addresses `SMALL_LAYOUT.size()` writable bytes.
        unsafe { ptr.as_ptr().write(0xA5) };
        // SAFETY: `ptr` came from `arena`; the larger layout has the same alignment.
        let grown = unsafe { arena.grow(ptr, SMALL_LAYOUT, LARGE_LAYOUT).unwrap() };
        black_box(grown);
        // SAFETY: `grown` came from `grow` with `LARGE_LAYOUT`.
        unsafe { arena.deallocate(grown.cast::<u8>(), LARGE_LAYOUT) };
    }
}

#[inline(never)]
pub fn allocator_grow_zeroed_in_place(arena: &Arena) {
    for _ in 0..N {
        let ptr = arena.allocate(SMALL_LAYOUT).unwrap().cast::<u8>();
        // SAFETY: `ptr` addresses `SMALL_LAYOUT.size()` writable bytes.
        unsafe { ptr.as_ptr().write(0xA5) };
        // SAFETY: `ptr` came from `arena`; the larger layout has the same alignment.
        let grown = unsafe { arena.grow_zeroed(ptr, SMALL_LAYOUT, LARGE_LAYOUT).unwrap() };
        black_box(grown);
        // SAFETY: `grown` came from `grow_zeroed` with `LARGE_LAYOUT`.
        unsafe { arena.deallocate(grown.cast::<u8>(), LARGE_LAYOUT) };
    }
}

#[inline(never)]
pub fn allocator_shrink_in_place(arena: &Arena) {
    for _ in 0..N {
        let ptr = arena.allocate(LARGE_LAYOUT).unwrap().cast::<u8>();
        // SAFETY: `ptr` addresses `LARGE_LAYOUT.size()` writable bytes.
        unsafe { ptr.as_ptr().write(0xA5) };
        // SAFETY: `ptr` came from `arena`; the smaller layout has the same alignment.
        let shrunk = unsafe { arena.shrink(ptr, LARGE_LAYOUT, SMALL_LAYOUT).unwrap() };
        black_box(shrunk);
        // SAFETY: `shrunk` came from `shrink` with `SMALL_LAYOUT`.
        unsafe { arena.deallocate(shrunk.cast::<u8>(), SMALL_LAYOUT) };
    }
}
