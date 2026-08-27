// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Every allocation entry point rejects a type aligned at or above the
//! arena's cap.
//!
//! Smart pointers recover their chunk header by masking the value
//! pointer's offset within its chunk tile. A value aligned at or above
//! the cap can land outside the first tile, where the mask recovers a
//! different chunk's header and `Drop` corrupts it. The guards make that
//! unreachable from safe code, and these tests hold every entry point to
//! it.
//!
//! The tests run against [`capped_arena`], whose caps are lowered so the
//! boundary is reachable by an alignment every codegen backend accepts.

use crate::Arena;
use crate::internal::constants::{CHUNK_ALIGN, max_smart_ptr_align};
use crate::tests_support::{ChunkOverAligned, SmartPtrOverAligned, SmartPtrOverAlignedDrop, capped_arena};

// The guards read their cap from the arena; `buffer_freezable` still reads
// `max_smart_ptr_align()` directly. The two must agree, or a `Vec` would
// freeze in place for an element the smart-pointer path rejects.
#[test]
fn default_caps_match_the_constants() {
    let arena = Arena::new();
    assert_eq!(arena.chunk_align_cap(), CHUNK_ALIGN);
    assert_eq!(arena.smart_ptr_align_cap(), max_smart_ptr_align());
}

#[test]
#[should_panic(expected = "the cap may only be lowered")]
fn set_align_cap_rejects_raising_the_cap() {
    Arena::new().set_align_cap(CHUNK_ALIGN * 2);
}

#[test]
fn rejection_reports_alignment_too_large() {
    let arena = capped_arena();
    let err = arena.try_alloc_with(|| SmartPtrOverAligned(0)).unwrap_err();
    assert!(err.is_alignment_too_large());

    let err = arena.try_alloc_arc_with(|| SmartPtrOverAlignedDrop(0)).unwrap_err();
    assert!(err.is_alignment_too_large());

    // A local binding, not `&[…][..]`: an over-aligned temporary that gets
    // const-promoted lands in a section the linker rejects.
    let src = [ChunkOverAligned(0)];
    let err = arena.try_alloc_slice_copy(&src[..]).unwrap_err();
    assert!(err.is_alignment_too_large());
}

#[test]
fn try_alloc_with_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_with(|| SmartPtrOverAligned(0)).unwrap_err();
}

#[test]
fn try_alloc_arc_with_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_arc_with(|| SmartPtrOverAlignedDrop(0)).unwrap_err();
}

#[test]
fn try_alloc_box_with_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_box_with(|| SmartPtrOverAlignedDrop(0)).unwrap_err();
}

#[test]
fn try_alloc_uninit_box_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_uninit_box::<SmartPtrOverAlignedDrop>().unwrap_err();
}

#[test]
fn try_alloc_uninit_arc_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_uninit_arc::<SmartPtrOverAlignedDrop>().unwrap_err();
}

#[test]
fn try_alloc_slice_copy_arc_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    let src = [SmartPtrOverAligned(0), SmartPtrOverAligned(1)];
    arena.try_alloc_slice_copy_arc(&src[..]).unwrap_err();
}

#[test]
fn try_alloc_arc_with_no_drop_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_arc_with(|| SmartPtrOverAligned(0)).unwrap_err();
}

#[test]
fn try_alloc_slice_fill_with_arc_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_slice_fill_with_arc(1, |_| SmartPtrOverAlignedDrop(0)).unwrap_err();
}

#[test]
fn try_alloc_slice_fill_with_arc_no_drop_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_slice_fill_with_arc(2, |_| SmartPtrOverAligned(0)).unwrap_err();
}

#[test]
fn try_alloc_slice_fill_with_box_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_slice_fill_with_box(1, |_| SmartPtrOverAlignedDrop(0)).unwrap_err();
}

#[test]
fn try_alloc_uninit_slice_arc_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_uninit_slice_arc::<SmartPtrOverAlignedDrop>(1).unwrap_err();
}

#[test]
fn try_alloc_uninit_slice_box_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_uninit_slice_box::<SmartPtrOverAlignedDrop>(1).unwrap_err();
}

// A pinned box hands out a smart pointer, so it rejects at the smart-pointer
// cap like the other box entry points.
#[test]
fn try_alloc_uninit_box_pin_rejects_smart_ptr_alignment() {
    let arena = capped_arena();
    arena.try_alloc_uninit_box_pin::<SmartPtrOverAligned>().unwrap_err();
}

// Simple-reference slices hand back a plain `&mut [T]` with no header
// recovery, so they use the looser chunk cap and only reject at it.
#[test]
fn try_alloc_slice_fill_with_rejects_chunk_alignment() {
    let arena = capped_arena();
    arena.try_alloc_slice_fill_with(1, |_| ChunkOverAligned(0)).unwrap_err();
}

#[test]
fn try_alloc_slice_copy_rejects_chunk_alignment() {
    let arena = capped_arena();
    let src = [ChunkOverAligned(0)];
    arena.try_alloc_slice_copy(&src[..]).unwrap_err();
}

#[test]
fn try_alloc_slice_fill_iter_rejects_chunk_alignment() {
    let arena = capped_arena();
    arena.try_alloc_slice_fill_iter((0..1).map(|_| ChunkOverAligned(0))).unwrap_err();
}

// A type aligned below the chunk cap is accepted by the simple-reference
// slice paths: the boundary the tighter smart-pointer cap does not apply to.
#[test]
fn alloc_slice_ref_accepts_below_chunk_alignment_for_non_drop() {
    let arena = capped_arena();
    let filled = arena.alloc_slice_fill_with(1, |_| SmartPtrOverAligned(0));
    assert_eq!(filled.len(), 1);

    let src = [SmartPtrOverAligned(0)];
    let cloned = arena.alloc_slice_clone(&src[..]);
    assert_eq!(cloned.len(), 1);
}

// Panicking entry points route through the same guards and surface the
// rejection as the arena's allocation-failure panic.
#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_with_panics_on_over_alignment() {
    let arena = capped_arena();
    let _ = arena.alloc_with(|| SmartPtrOverAligned(0));
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_arc_with_panics_on_over_alignment() {
    let arena = capped_arena();
    let _ = arena.alloc_arc_with(|| SmartPtrOverAligned(0));
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_box_with_panics_on_over_alignment() {
    let arena = capped_arena();
    let _ = arena.alloc_box_with(|| SmartPtrOverAligned(0));
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_uninit_box_panics_on_over_alignment() {
    let arena = capped_arena();
    let _ = arena.alloc_uninit_box::<SmartPtrOverAligned>();
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_uninit_arc_panics_on_over_alignment() {
    let arena = capped_arena();
    let _ = arena.alloc_uninit_arc::<SmartPtrOverAligned>();
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_copy_panics_on_over_alignment() {
    let arena = capped_arena();
    let src = [ChunkOverAligned(0)];
    let _ = arena.alloc_slice_copy(&src[..]);
}
