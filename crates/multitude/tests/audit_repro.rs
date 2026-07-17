// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Correctness tests for allocator layout, initialization, and ZST handouts.

#![allow(clippy::std_instead_of_core, reason = "test code")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::clone_on_ref_ptr, reason = "test code")]
#![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
#![allow(clippy::large_stack_arrays, reason = "test code")]
#![allow(clippy::large_types_passed_by_value, reason = "test code")]
#![allow(
    clippy::redundant_clone,
    reason = "explicit clones in #[should_panic] tests keep the counter visible after the panic"
)]

use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use allocator_api2::alloc::{Allocator, Global};
use multitude::{Arc, Arena};

struct DropCounter(StdArc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct LargeAllocator {
    _state: [u8; 60 * 1024],
}

unsafe impl Allocator for LargeAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded under the caller's Allocator contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[repr(align(8192))]
struct HighlyAligned([u8; 8192]);

/// Chunk headers must remain in the first 64-KiB tile regardless of the
/// backing allocator's state size. Storing `A` inline in every chunk made the
/// header unbounded and caused smart-pointer drop to recover a false header.
#[test]
fn large_allocator_state_does_not_displace_smart_pointer_payloads() {
    let allocator = LargeAllocator { _state: [0; 60 * 1024] };
    let (boxed, arc, rc) = {
        let arena = Arena::new_in(allocator);
        (
            arena.alloc_box(HighlyAligned([1; 8192])),
            arena.alloc_arc(HighlyAligned([2; 8192])),
            arena.alloc_rc(HighlyAligned([3; 8192])),
        )
    };

    assert_eq!(boxed.0[0], 1);
    assert_eq!(arc.0[0], 2);
    assert_eq!(rc.0[0], 3);
}

/// `Box::drop` calls `drop_in_place::<T>(ptr)`, so for `T = U` after
/// `assume_init`, the value's `Drop` *does* run via `drop_in_place`
/// regardless of any chunk drop list.
#[test]
fn alloc_box_of_maybeuninit_assume_init_drops_inner() {
    let counter = StdArc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let b_uninit = arena.alloc_box(MaybeUninit::new(DropCounter(counter.clone())));
        let b = unsafe { b_uninit.assume_init() };
        drop(b);
    }
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

/// With per-`Arc` reference counting, `alloc_arc(MaybeUninit::new(x))`
/// followed by `assume_init` works correctly: `Arc::drop` runs the inner
/// value's destructor eagerly on the last clone without a chunk drop entry.
#[test]
fn alloc_arc_of_maybeuninit_assume_init_drops_inner() {
    let counter = StdArc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let arc_uninit = arena.alloc_arc(MaybeUninit::new(DropCounter(counter.clone())));
        let arc = unsafe { arc_uninit.assume_init() };
        drop(arc);
    }
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

/// `arena.alloc_uninit_arc::<U>()` followed by `assume_init` reserves the
/// drop entry up front, so this pattern works correctly.
#[test]
fn alloc_uninit_arc_assume_init_drops_inner() {
    let counter = StdArc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let arc_uninit = arena.alloc_uninit_arc::<DropCounter>();
        unsafe {
            Arc::as_ptr(&arc_uninit)
                .cast_mut()
                .write(MaybeUninit::new(DropCounter(counter.clone())));
        }
        let arc = unsafe { arc_uninit.assume_init() };
        drop(arc);
    }
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

/// Concurrent `assume_init` calls publish initialization metadata atomically.
///
/// Run under Miri to detect metadata races:
/// `MIRIFLAGS="-Zmiri-many-seeds=0..16" cargo +nightly miri test ...`
#[test]
fn arc_concurrent_assume_init_no_race() {
    let arena = Arena::new();
    let counter = StdArc::new(AtomicUsize::new(0));

    let a = arena.alloc_uninit_arc::<DropCounter>();
    unsafe {
        Arc::as_ptr(&a).cast_mut().write(MaybeUninit::new(DropCounter(counter.clone())));
    }
    let b = a.clone();

    let h1 = std::thread::spawn(move || {
        let _x = unsafe { a.assume_init() };
    });
    let h2 = std::thread::spawn(move || {
        let _y = unsafe { b.assume_init() };
    });
    h1.join().unwrap();
    h2.join().unwrap();

    drop(arena);
    assert_eq!(
        counter.load(Ordering::Relaxed),
        1,
        "DropCounter::drop must run exactly once across both clones"
    );
}

/// Reference slices accept non-Drop types aligned below `CHUNK_ALIGN`.
///
/// We use a ZST with `#[repr(align(32768))]` so the type's alignment
/// checks the cap without forcing a 32 KiB stack frame.
#[cfg(not(utc_backend))]
#[test]
fn alloc_slice_ref_accepts_half_chunk_alignment_for_non_drop() {
    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct Wide;
    let arena = Arena::new();
    let s = arena.alloc_slice_fill_with::<Wide, _>(1, |_| Wide);
    assert_eq!(s.len(), 1);

    let src: &[Wide] = &[Wide];
    let c = arena.alloc_slice_clone::<Wide>(src);
    assert_eq!(c.len(), 1);
}

/// Each refcounted ZST handout reserves a distinct one-byte tag, bounding
/// per-chunk handouts and preserving refcount surplus invariants.
#[test]
fn zst_shared_handouts_advance_cursor() {
    let arena = Arena::new();

    let a = arena.alloc_arc(());
    let b = arena.alloc_arc(());
    let c = arena.alloc_arc(());
    assert_ne!(a.as_ptr(), b.as_ptr(), "ZST Arc handouts must get distinct addresses");
    assert_ne!(b.as_ptr(), c.as_ptr(), "ZST Arc handouts must get distinct addresses");

    let bx1 = arena.alloc_box(());
    let bx2 = arena.alloc_box(());
    assert_ne!(bx1.as_ptr(), bx2.as_ptr(), "ZST Box handouts must get distinct addresses");

    // Fill and refill the starter chunk without an excessive Miri loop.
    for _ in 0..600 {
        drop(arena.alloc_arc(()));
        drop(arena.alloc_box(()));
    }

    let z = arena.alloc_arc(());
    let _ = arena.alloc_arc(7_u64);
    assert!(!z.as_ptr().is_null());
    drop(arena);
}
