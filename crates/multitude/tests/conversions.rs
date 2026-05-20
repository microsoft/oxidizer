// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the conversion methods between owned and refcounted smart pointers:
//!
//! - [`ArenaBox::into_rc`] — convert owned box to shared rc, no copy.
//! - [`ArenaBoxStr::into_rc_str`] — same as `into_rc` for the str type.
//!
//! Slice-specific [`ArenaBox<[T]>::into_rc`] is tested in
//! `tests/dst_box.rs` (cfg-gated on the `dst` feature).
//!
//! There are intentionally no `leak()` methods on the owned arena
//! smart pointers — see the comments in `src/box` and
//! `src/box_str` for the architectural reason
//! (chunk-refcount-vs-pinning mismatch).

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use core::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use multitude::Arena;

#[test]
fn arena_box_into_rc_basic() {
    let arena = Arena::new();
    let b = arena.alloc_box(42_u32);
    let r = b.into_rc();
    assert_eq!(*r, 42);
}

#[test]
fn arena_box_into_rc_preserves_chunk_refcount() {
    // The chunk's +1 from the box must transfer to the rc, not double-count.
    // We allocate, convert, clone the rc once, drop both rc smart pointers, and
    // verify the arena keeps working (chunk reclaims correctly).
    let arena = Arena::new();
    let b = arena.alloc_box(123_u32);
    let r = b.into_rc();
    let r2 = r.clone();
    drop(r);
    drop(r2);
    // Arena still works after the chunk reclaims.
    let v = arena.alloc_rc(456_u32);
    assert_eq!(*v, 456);
}

#[test]
fn arena_box_into_rc_drops_value_at_chunk_teardown() {
    // When converted to rc, the value's Drop should run at chunk teardown,
    // NOT immediately on the original box's drop. Verify by tracking drops.
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    struct Tracked(u32);
    impl Drop for Tracked {
        fn drop(&mut self) {
            let _ = COUNT.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    COUNT.store(0, AtomicOrdering::SeqCst);
    let arena = Arena::new();
    let b = arena.alloc_box(Tracked(7));
    // Conversion should NOT trigger drop.
    let r = b.into_rc();
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(r.0, 7);
    // Drop the rc smart pointer. The chunk teardown runs the value's drop.
    drop(r);
    drop(arena);
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn arena_box_into_rc_outlives_arena() {
    // ArenaRc smart pointers can outlive the arena; ArenaBox cannot. Verify the
    // converted smart pointer inherits the rc's outlive-arena property.
    let r = {
        let arena = Arena::new();
        let b = arena.alloc_box("outlives".to_string());
        b.into_rc()
    };
    assert_eq!(*r, "outlives");
}

#[test]
fn arena_box_into_rc_no_double_drop() {
    // Make sure the value's destructor runs exactly once across the
    // conversion + chunk teardown sequence.
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    struct Tracked;
    impl Drop for Tracked {
        fn drop(&mut self) {
            let _ = COUNT.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    COUNT.store(0, AtomicOrdering::SeqCst);
    {
        let arena = Arena::new();
        let b = arena.alloc_box(Tracked);
        let _r = b.into_rc();
    }
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 1, "value must drop exactly once");
}

#[test]
fn arena_box_into_rc_for_copy_type_no_drop() {
    // T: Copy means no DropEntry was registered for this value, but the
    // chunk's +1 refcount still transfers cleanly.
    let arena = Arena::new();
    let b = arena.alloc_box(42_u32);
    let r = b.into_rc();
    let r2 = r.clone();
    drop(r);
    drop(r2);
    let v = arena.alloc_rc(99_u32);
    assert_eq!(*v, 99);
}

#[test]
fn arena_box_str_into_rc_str_basic() {
    let arena = Arena::new();
    let b = arena.alloc_str_box("hello");
    let s = b.into_rc_str();
    assert_eq!(&*s, "hello");
}

#[test]
fn arena_box_str_into_rc_str_after_mutation() {
    let arena = Arena::new();
    let mut b = arena.alloc_str_box("hello");
    b.make_ascii_uppercase();
    let s = b.into_rc_str();
    assert_eq!(&*s, "HELLO");
}

#[test]
fn arena_box_str_into_rc_str_clone_works() {
    let arena = Arena::new();
    let b = arena.alloc_str_box("shareable");
    let s = b.into_rc_str();
    let s2 = s.clone();
    let s3 = s.clone();
    drop(s);
    drop(s2);
    assert_eq!(&*s3, "shareable");
}

#[test]
fn arena_box_str_into_rc_str_outlives_arena() {
    let s = {
        let arena = Arena::new();
        let b = arena.alloc_str_box("survives");
        b.into_rc_str()
    };
    assert_eq!(&*s, "survives");
}

#[test]
fn arena_box_str_into_rc_str_empty_string() {
    let arena = Arena::new();
    let b = arena.alloc_str_box("");
    let s = b.into_rc_str();
    assert_eq!(&*s, "");
    assert_eq!(s.len(), 0);
}

#[test]
fn arena_box_str_into_rc_str_preserves_size_invariant() {
    // Size of the resulting smart pointer should still be one pointer (the
    // single-pointer compactness is preserved across conversion).
    use multitude::strings::RcStr;
    assert_eq!(size_of::<RcStr>(), size_of::<usize>());
}

#[test]
fn arena_box_into_rc_zst() {
    // ZST with no Drop — no DropEntry is reserved.
    #[derive(Debug)]
    struct Zst;
    let arena = Arena::new();
    let b = arena.alloc_box(Zst);
    let r = b.into_rc();
    let r2 = r.clone();
    drop(r);
    drop(r2);
    // Arena still works.
    let v = arena.alloc_rc(99_u32);
    assert_eq!(*v, 99);
}

#[test]
fn arena_box_into_rc_zst_with_drop() {
    // ZST WITH Drop — DropEntry IS reserved. Verify Drop runs exactly
    // once across the box → rc → teardown lifecycle.
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    struct ZstDrop;
    impl Drop for ZstDrop {
        fn drop(&mut self) {
            let _ = COUNT.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    COUNT.store(0, AtomicOrdering::SeqCst);
    {
        let arena = Arena::new();
        let b = arena.alloc_box(ZstDrop);
        let _r = b.into_rc();
    }
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn arena_box_into_rc_high_alignment_value() {
    // High-alignment T exercises the over-aligned DropEntry slot in
    // alloc_with_drop_entry_unchecked. Conversion must preserve the
    // entry's location so chunk teardown can find it correctly.
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    #[repr(align(64))]
    #[derive(Debug)]
    struct AlignedDrop {
        _data: [u8; 16],
    }
    impl Drop for AlignedDrop {
        fn drop(&mut self) {
            let _ = COUNT.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    COUNT.store(0, AtomicOrdering::SeqCst);
    {
        let arena = Arena::new();
        // Force the bump cursor away from a 64-aligned position.
        let _decoy = arena.alloc(0_u8);
        let b = arena.alloc_box(AlignedDrop { _data: [0xAB; 16] });
        let r = b.into_rc();
        // Drop the rc; chunk teardown will run the Drop via the entry
        // it locates from the value pointer's address.
        drop(r);
    }
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn arena_box_str_into_rc_str_round_trip_many_strings() {
    // Stress: many strings, each converted box → rc → drop. Verify the
    // arena stays healthy across hundreds of refcount transfers.
    let arena = Arena::new();
    let mut handles = std::vec::Vec::new();
    for i in 0..200 {
        let b = arena.alloc_str_box(format!("string-{i}"));
        handles.push(b.into_rc_str());
    }
    assert_eq!(&*handles[0], "string-0");
    assert_eq!(&*handles[199], "string-199");
    // Random-order drop.
    handles.reverse();
    for h in handles {
        let _ = h.len();
    }
}

#[test]
fn arena_box_into_rc_then_back_to_arena_works() {
    // After all conversion-derived rc smart pointers drop, the arena's chunks
    // reclaim via the cache. Subsequent allocations should reuse them.
    let arena = Arena::new();
    {
        let b = arena.alloc_box(42_u32);
        let r = b.into_rc();
        drop(r);
    }
    {
        // Reuse the same chunk slot.
        let b = arena.alloc_box(99_u32);
        let r = b.into_rc();
        assert_eq!(*r, 99);
    }
}

/// Regression: `Box::into_rc` used to install a drop entry via
/// `install_drop_entry_local` that bumped the chunk's `drop_count` but
/// did NOT update the arena's `current_local.drop_back` mirror. The
/// next allocation on the same chunk then reused the same back-stack
/// slot, corrupting the drop list and segfaulting on chunk teardown.
///
/// Fix: `alloc_box<T>` now eagerly reserves a `noop_drop_shim` entry
/// for every `T: needs_drop`, and `Box::into_rc` retargets that entry
/// to `drop_shim_one::<T>` rather than installing a new one.
#[test]
fn arena_box_into_rc_does_not_corrupt_drop_list() {
    use core::sync::atomic::{AtomicUsize, Ordering};

    static DROPS: [AtomicUsize; 4] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

    struct DropProbe<const ID: usize>;
    impl<const ID: usize> Drop for DropProbe<ID> {
        fn drop(&mut self) {
            DROPS[ID].fetch_add(1, Ordering::SeqCst);
        }
    }

    for d in &DROPS {
        d.store(0, Ordering::SeqCst);
    }
    {
        let arena = Arena::new();
        // Step 1: box-allocate a Drop value. Reserves a noop entry in the chunk.
        let b: multitude::Box<DropProbe<0>> = arena.alloc_box(DropProbe::<0>);
        // Step 2: rc-allocate a different Drop value. Reserves a separate entry
        // (advances arena's drop_back mirror).
        let _r1: multitude::Rc<DropProbe<1>> = arena.alloc_rc(DropProbe::<1>);
        // Step 3: convert the box to rc. With the pre-fix code this would
        // install a NEW entry by walking the chunk's drop_count, colliding
        // with the slot reserved in step 2. With the fix, it retargets the
        // entry reserved in step 1.
        let _rc0: multitude::Rc<DropProbe<0>> = b.into_rc();
        // Step 4: another rc allocation. With the pre-fix code, this would
        // re-reuse the same slot as step 3 → drop-list corruption and a
        // segfault at chunk teardown. With the fix, it gets its own slot.
        let _r2: multitude::Rc<DropProbe<2>> = arena.alloc_rc(DropProbe::<2>);
    }
    // Each Drop value should run its destructor exactly once.
    assert_eq!(DROPS[0].load(Ordering::SeqCst), 1, "DropProbe<0> dropped wrong number of times");
    assert_eq!(DROPS[1].load(Ordering::SeqCst), 1, "DropProbe<1> dropped wrong number of times");
    assert_eq!(DROPS[2].load(Ordering::SeqCst), 1, "DropProbe<2> dropped wrong number of times");
    assert_eq!(DROPS[3].load(Ordering::SeqCst), 0, "DropProbe<3> should not have been allocated");
}
