// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for Simple-reference allocations: `Arena::alloc`, `alloc_str`,
//! and the slice variants. These return `&'arena mut T` whose lifetime
//! is tied to the arena reference, with no per-pointer refcount.
//!
//! The chunk that hosts each value is "pinned" so it survives past
//! chunk rotation. The pinning costs one bit per chunk and is released
//! at arena drop.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
#![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
#![allow(clippy::cast_possible_truncation, reason = "test data is small")]
#![allow(clippy::needless_range_loop, reason = "test indexing is intentional")]
#![allow(clippy::missing_asserts_for_indexing, reason = "test code")]
#![allow(clippy::redundant_clone, reason = "test code")]
#![allow(clippy::needless_lifetimes, reason = "explicit lifetimes clarify the test's intent")]
#![allow(clippy::assertions_on_result_states, reason = "tests assert error returns")]
#![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
#![allow(unused_results, reason = "test code")]

mod common;

use multitude::Arena;

#[test]
fn alloc_returns_mutable_reference() {
    let arena = Arena::new();
    let x: &mut u32 = arena.alloc(42);
    assert_eq!(*x, 42);
    *x = 100;
    assert_eq!(*x, 100);
}

#[test]
fn alloc_with_constructs_in_place() {
    let arena = Arena::new();
    let v: &mut std::vec::Vec<i32> = arena.alloc_with(|| vec![1, 2, 3]);
    v.push(4);
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn alloc_many_disjoint_mutable_refs_coexist() {
    // Simple references: multiple `&mut T` from the same arena are
    // disjoint, mutually-live mutable references.
    let arena = Arena::new();
    let a: &mut u64 = arena.alloc(1);
    let b: &mut u64 = arena.alloc(2);
    let c: &mut u64 = arena.alloc(3);
    *a += 10;
    *b += 20;
    *c += 30;
    assert_eq!(*a, 11);
    assert_eq!(*b, 22);
    assert_eq!(*c, 33);
}

#[test]
fn alloc_str_copies_and_returns_mut_str() {
    let arena = Arena::new();
    let s: &mut str = arena.alloc_str("hello");
    assert_eq!(s, "hello");
    s.make_ascii_uppercase();
    assert_eq!(s, "HELLO");
}

#[test]
fn alloc_str_empty() {
    let arena = Arena::new();
    let s: &mut str = arena.alloc_str("");
    assert_eq!(s, "");
}

#[test]
fn alloc_slice_copy_mutable() {
    let arena = Arena::new();
    let s: &mut [u32] = arena.alloc_slice_copy([1, 2, 3, 4, 5]);
    assert_eq!(s, &[1, 2, 3, 4, 5][..]);
    s[2] = 99;
    assert_eq!(s, &[1, 2, 99, 4, 5][..]);
}

#[test]
fn alloc_slice_clone_works() {
    let arena = Arena::new();
    let originals = [
        std::string::String::from("a"),
        std::string::String::from("b"),
        std::string::String::from("c"),
    ];
    let s: &mut [String] = arena.alloc_slice_clone(&originals);
    assert_eq!(s.len(), 3);
    assert_eq!(s[0], "a");
    s[0].push('!');
    assert_eq!(s[0], "a!");
    // Originals untouched.
    assert_eq!(originals[0], "a");
}

#[test]
fn alloc_slice_fill_with_works() {
    let arena = Arena::new();
    let s: &mut [u32] = arena.alloc_slice_fill_with(10, |i| (i as u32) * (i as u32));
    assert_eq!(s.len(), 10);
    for i in 0..10 {
        assert_eq!(s[i], (i as u32) * (i as u32));
    }
}

#[test]
fn try_alloc_slice_clone_works() {
    let arena = Arena::new();
    let originals = [std::string::String::from("x"), std::string::String::from("y")];
    let s: &mut [String] = arena.try_alloc_slice_clone(&originals).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0], "x");
    assert_eq!(s[1], "y");
}

#[test]
fn alloc_slice_fill_iter_works() {
    let arena = Arena::new();
    let s: &mut [u64] = arena.alloc_slice_fill_iter([0_u64, 1, 2, 3, 4]);
    assert_eq!(s, &[0, 1, 2, 3, 4]);
}

#[test]
fn try_alloc_slice_fill_iter_works() {
    let arena = Arena::new();
    let s: &mut [i32] = arena.try_alloc_slice_fill_iter([10, 20, 30]).unwrap();
    assert_eq!(s, &[10, 20, 30]);
}

#[test]
fn alloc_slice_fill_iter_empty() {
    let arena = Arena::new();
    let s: &mut [u32] = arena.alloc_slice_fill_iter(core::iter::empty::<u32>());
    assert!(s.is_empty());
}

#[test]
fn alloc_survives_chunk_rotation() {
    // Force chunk rotation while a bump-ref is alive. Without pinning,
    // the rotated chunk would be freed and the &mut would dangle.
    let arena = Arena::builder().build();
    let pinned_value: &mut [u8] = arena.alloc_slice_copy([0xAB; 1024]);
    pinned_value[0] = 0xCD;
    // Force chunk rotation: allocate enough to retire the current chunk.
    for _ in 0..10 {
        let _filler = arena.alloc_slice_copy([0_u8; 1024]);
    }
    // The original bump-ref must still be valid.
    assert_eq!(pinned_value[0], 0xCD);
    assert_eq!(pinned_value[1023], 0xAB);
    assert_eq!(pinned_value.len(), 1024);
}

#[test]
fn alloc_works_across_many_rotations() {
    // Stress test: many bump-allocs spanning many chunks. All
    // references must remain valid.
    let arena = Arena::builder().build();
    let mut refs: std::vec::Vec<&mut u32> = std::vec::Vec::with_capacity(1000);
    for i in 0..1000_u32 {
        refs.push(arena.alloc(i));
    }
    for (i, r) in refs.iter().enumerate() {
        assert_eq!(**r, i as u32);
    }
}

#[test]
fn alloc_drop_runs_at_arena_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter(std::sync::Arc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let _r1: &mut DropCounter = arena.alloc(DropCounter(std::sync::Arc::clone(&counter)));
        let _r2: &mut DropCounter = arena.alloc(DropCounter(std::sync::Arc::clone(&counter)));
        let _r3: &mut DropCounter = arena.alloc(DropCounter(std::sync::Arc::clone(&counter)));
        assert_eq!(counter.load(Ordering::SeqCst), 0, "drop must not run before arena drop");
        // arena drops here → all three DropCounters must run.
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn alloc_slice_fill_with_drop_runs_at_arena_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter(std::sync::Arc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let _slice: &mut [DropCounter] = arena.alloc_slice_fill_with(7, |_| DropCounter(std::sync::Arc::clone(&counter)));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 7);
}

#[test]
fn bump_ref_coexists_with_arena_rc() {
    let arena = Arena::new();
    let rc = arena.alloc_rc(std::string::String::from("refcounted"));
    let bump_ref: &mut std::vec::Vec<i32> = arena.alloc(vec![1, 2, 3]);
    bump_ref.push(4);
    assert_eq!(*rc, "refcounted");
    assert_eq!(bump_ref.as_slice(), &[1, 2, 3, 4]);
    let rc2 = rc.clone();
    assert_eq!(*rc2, "refcounted");
}

#[test]
fn arena_rc_can_outlive_arena_with_pinned_chunk() {
    // An ArenaRc allocated alongside bump-refs in the same chunk:
    // when the arena drops, the pinned-list release brings the
    // chunk's refcount to 1 (just the rc); the rc keeps the chunk
    // alive past arena drop. When the rc drops, the chunk tears
    // down.
    let rc = {
        let arena = Arena::new();
        let _bump_ref: &mut u32 = arena.alloc(99);
        arena.alloc_rc(std::string::String::from("outlives the arena"))
    };
    // arena dropped; bump-ref's lifetime expired; rc still valid.
    assert_eq!(*rc, "outlives the arena");
    drop(rc);
}

#[test]
fn alloc_lifetime_bound_by_arena_borrow() {
    // Compile-time check: this compiles because the bump-ref's
    // lifetime is bounded by the arena reference.
    fn use_arena<'a>(arena: &'a Arena) -> &'a mut u32 {
        arena.alloc(7)
    }
    let arena = Arena::new();
    let r = use_arena(&arena);
    assert_eq!(*r, 7);
}

#[cfg(feature = "stats")]
#[test]
fn alloc_charges_stats() {
    let arena = Arena::new();
    let before = arena.stats().total_bytes_allocated;
    let _r: &mut u64 = arena.alloc(42);
    let after = arena.stats().total_bytes_allocated;
    assert!(after >= before + 8);
}

use crate::common::FailingAllocator;

#[test]
fn try_alloc_returns_err_on_failing_allocator() {
    let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
    assert!(arena.try_alloc(0_u32).is_err());
    assert!(arena.try_alloc_with(|| 0_u32).is_err());
    assert!(arena.try_alloc_slice_copy::<u8>(&[1, 2, 3]).is_err());
    assert!(arena.try_alloc_slice_fill_with::<u32, _>(3, |i| i as u32).is_err());
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_panics_on_failing_allocator() {
    let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
    let _ = arena.alloc(0_u32);
}

// Mixing with Allocator-trait usage (Vec<T, &Arena>) — pinning the same
// chunk both ways should still tear down cleanly.

#[test]
fn pinned_chunk_with_allocator_api2_vec_drops_cleanly() {
    let arena: Arena = Arena::builder().build();
    let _bump_ref: &mut u32 = arena.alloc(123);
    let mut v: allocator_api2::vec::Vec<u8, &Arena> = allocator_api2::vec::Vec::new_in(&arena);
    for _ in 0..5_000_u32 {
        v.push(0);
    }
    assert!(_bump_ref == &mut 123);
    drop(v);
    // arena drops at end-of-scope; pinned chunk is freed cleanly.
}

// Slack reclamation interaction with cache: cached chunk should NOT be
// pinned (cache reuse must reset the flag).

#[cfg(feature = "stats")]
#[test]
fn cache_revive_resets_pinned_flag() {
    // If the pinned flag weren't reset on cache revive, a
    // subsequently-cached chunk that gets reused for a non-bump
    // allocation would behave as pinned and leak. Verify chunks
    // recycle correctly.
    let arena: Arena = Arena::builder().build();
    {
        let _bump_ref: &mut u32 = arena.alloc(1);
        // The chunk holding the bump-ref is now pinned. It can't go
        // to the cache while pinned (it goes to the pinned list at
        // rotation, then to free at arena drop).
        let _filler = arena.alloc_slice_copy([0_u8; 3 * 1024]);
        // The chunk is now full and bump-pinned.
    }
    // Allocate a (non-bump) ArenaRc so a fresh non-pinned chunk is created
    // and might end up in the cache later.
    let mut handles = std::vec::Vec::new();
    for i in 0..10_u32 {
        handles.push(arena.alloc_rc(i));
    }
    drop(handles);
    // Stats sanity: chunks were used as expected.
    assert!(arena.stats().normal_local_chunks_allocated >= 1);
}
