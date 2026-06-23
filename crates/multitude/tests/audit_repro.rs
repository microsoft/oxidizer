// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reproducer tests for findings from the correctness audit. Each test
//! fails on the original code and passes after the corresponding fix.

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

use core::mem::MaybeUninit;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use multitude::{Arc, Arena};

struct DropCounter(StdArc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Same leak, Box variant: `arena.alloc_box(MaybeUninit::new(x)).assume_init()`.
///
/// `Box::drop` calls `drop_in_place::<T>(ptr)`, so for `T = U` after
/// `assume_init`, the value's `Drop` *does* run via `drop_in_place`
/// regardless of any chunk drop list — so this case is sound.
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
/// value's destructor eagerly on the last clone (no chunk drop entry is
/// involved), so the previously-unsupported pattern is now sound.
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

/// Audit finding #1: `Arc::<MaybeUninit<T>>::assume_init` writes the
/// drop-entry's `drop_fn` field non-atomically. Two threads each
/// holding a clone of the same allocation can race on that store.
///
/// The reproducer launches two threads that concurrently call
/// `assume_init` on cloned `Arc<MaybeUninit<DropCounter>>` handles.
/// Without atomic stores, this is a write/write data race on the
/// same memory location and Miri (or TSAN) flags it. After the fix
/// the field is published atomically and the test passes cleanly
/// under Miri.
///
/// Run under Miri to actually catch the race:
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

/// Audit finding #5: `try_alloc_slice_local_no_drop_with` uses
/// `MAX_SMART_PTR_ALIGN` (32 KiB) as the alignment cap even when the
/// caller is a `SimpleRef` slice path. The Copy slice sibling already
/// uses `CHUNK_ALIGN` (64 KiB), and the documented cap on
/// `alloc_slice_fill_with` is 64 KiB. A 32 KiB-aligned non-Drop type
/// must succeed via `alloc_slice_fill_with` / `alloc_slice_clone`.
///
/// We use a ZST with `#[repr(align(32768))]` so the type's alignment
/// exercises the cap without forcing a 32 KiB stack frame (Windows
/// MSVC chokes on rustc-emitted stack alignment of that size; see
/// the `HalfChunkAlign` / `ChunkAlign` note in `coverage_arena_gaps.rs`).
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

/// Audit finding: non-`Drop` ZST `alloc_arc` / `alloc_box` handouts did
/// not advance the bump cursor (`try_alloc(0, _)` is a cursor no-op), so
/// a single chunk could hand out unbounded refcounted handles. Each
/// handout draws down the pre-credited shared-ref surplus via the
/// non-atomic `local_shared_count`; an unbounded run exhausts it,
/// driving the chunk's atomic refcount to zero while the chunk is still
/// installed (use-after-free) or underflowing the surplus reconciliation
/// at retire (double-free). The fix reserves a 1-byte tag per such
/// handout so the cursor advances and per-chunk handouts stay bounded by
/// the chunk capacity (far below the surplus).
///
/// Deterministic regression proxy: consecutive ZST shared handouts must
/// occupy distinct addresses (pre-fix they shared one address and the
/// cursor never moved). The create-and-drop loop exercises the refills
/// the tag now forces and confirms the arena stays consistent afterward.
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

    // A few hundred create-and-drop cycles still force the (512-byte
    // starter) chunk to fill (1 byte each) and refill at least once. Pre-fix
    // the cursor never advanced, so this pattern could drive the live
    // chunk's atomic refcount to zero. A few hundred iterations exercise the
    // refill the tag now forces without a multi-thousand Miri loop.
    for _ in 0..600 {
        drop(arena.alloc_arc(()));
        drop(arena.alloc_box(()));
    }

    // Arena remains usable for references and smart pointers after the churn.
    let z = arena.alloc_arc(());
    let _ = arena.alloc_arc(7_u64);
    assert!(!z.as_ptr().is_null());
    drop(arena);
}
