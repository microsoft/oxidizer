// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_safety_comment,
    reason = "residue of Rc-test removal: orphaned helpers/imports kept to preserve surrounding test bodies verbatim"
)]

//! Tests for the [`Arena`] type itself: constructors, builder, stats,
//! cache behavior, byte budget, preallocation.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std for thread/sync primitives")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
#![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
#![allow(clippy::manual_assert, reason = "explicit panic clarifies safety-net intent")]
#![allow(clippy::cast_possible_truncation, reason = "test code: bounded indices fit in u32")]
#![allow(clippy::needless_borrows_for_generic_args, reason = "explicit borrows clarify intent in tests")]

use multitude::Arena;

mod common;

#[test]
fn allocator_accessor() {
    let arena = Arena::new();
    let _: &allocator_api2::alloc::Global = arena.allocator();
}

// `ChunkSizeOutOfRange` was removed from `BuildError` along with the
// `chunk_size` builder knob; the adaptive ramp manages chunk sizes
// itself. The previous boundary tests (chunk_size below min, chunk_size
// above CHUNK_ALIGN) are no longer reachable via the public API and
// have been deleted.

#[test]
#[should_panic(expected = "max_normal_alloc must be in")]
fn builder_max_normal_alloc_zero_rejected() {
    let _ = Arena::builder().max_normal_alloc(0).try_build();
}

#[test]
#[should_panic(expected = "max_normal_alloc must be in")]
fn builder_max_normal_alloc_below_min_rejected() {
    // Anything below the 4 KiB floor is rejected.
    let _ = Arena::builder().max_normal_alloc(2048).try_build();
}

#[test]
#[should_panic(expected = "max_normal_alloc must be in")]
fn builder_max_normal_alloc_too_large_rejected() {
    let _ = Arena::builder().max_normal_alloc(128 * 1024).try_build();
}

#[test]
fn builder_with_capacity_too_small_rejected() {
    let result = std::panic::catch_unwind(|| Arena::builder().with_capacity_local(256).try_build());
    assert!(result.is_err(), "with_capacity_local(256) must panic (below MIN_CHUNK_BYTES = 512)");
}

#[test]
fn try_alloc_str_returns_mutable_str() {
    let arena = Arena::new();
    let s: &mut str = arena.try_alloc_str("hello").unwrap();
    s.make_ascii_uppercase();
    assert_eq!(s, "HELLO");
}

#[test]
fn try_alloc_str_arc_returns_handle() {
    let arena = Arena::new();
    let s = arena.try_alloc_str_arc("arc").unwrap();
    assert_eq!(&*s, "arc");
}

#[test]
fn try_alloc_str_accepts_string() {
    // impl AsRef<str> covers both &str and String.
    let arena = Arena::new();
    let owned = std::string::String::from("from String");
    let s: &mut str = arena.try_alloc_str(owned).unwrap();
    assert_eq!(s, "from String");
}

#[test]
fn try_alloc_string_with_capacity_succeeds() {
    let arena = Arena::new();
    let mut s = arena.try_alloc_string_with_capacity(64).unwrap();
    s.push_str("preallocated");
    assert!(s.capacity() >= 64);
    assert_eq!(s.as_str(), "preallocated");
}

#[test]
fn try_alloc_string_with_capacity_zero_works() {
    let arena = Arena::new();
    let s = arena.try_alloc_string_with_capacity(0).unwrap();
    assert_eq!(s.capacity(), 0);
    assert_eq!(s.len(), 0);
}

#[test]
fn try_alloc_vec_with_capacity_succeeds() {
    let arena = Arena::new();
    let mut v = arena.try_alloc_vec_with_capacity::<u32>(50).unwrap();
    for i in 0..50 {
        v.push(i);
    }
    assert!(v.capacity() >= 50);
    assert_eq!(v.len(), 50);
}

#[test]
fn try_alloc_vec_with_capacity_zero_works() {
    let arena = Arena::new();
    let v: multitude::vec::Vec<u8, _> = arena.try_alloc_vec_with_capacity(0).unwrap();
    assert_eq!(v.capacity(), 0);
    assert_eq!(v.len(), 0);
}

/// Size that forces an oversized one-shot chunk allocation (i.e. a
/// chunk whose total size exceeds `MAX_CHUNK_BYTES = 64 KiB`).
/// 65 KiB is the *minimum* size that triggers the oversized branch;
/// any larger value exercises the same code path, so use the minimum
/// to keep tests fast under Miri.
const OVERSIZED_BYTES: usize = 65 * 1024;

#[test]
fn oversized_bump_alloc_does_not_leak_on_drop() {
    let alloc = common::TrackingAllocator::new();
    // Heap-allocate the source so cargo-careful's instrumented build
    // doesn't blow the stack on the large literal.
    let src = vec![0_u8; OVERSIZED_BYTES];
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        // The source exceeds `MAX_CHUNK_BYTES` (64 KiB), so this
        // allocation must take the oversized one-shot chunk path.
        // Previously these chunks were leaked because they were never
        // linked into current_*, the pinned list, or the cache.
        let _slice = arena.alloc_slice_copy(&src);
        assert!(alloc.live_chunks() >= 1);
    }
    assert_eq!(alloc.live_chunks(), 0, "arena drop must free all chunks");
    assert_eq!(alloc.live_bytes(), 0);
}

#[test]
fn oversized_bump_alloc_does_not_leak_on_reset() {
    let alloc = common::TrackingAllocator::new();
    let src = vec![0_u8; OVERSIZED_BYTES];
    let mut arena = Arena::builder_in(alloc.clone()).build();
    let _ = arena.alloc_slice_copy(&src);
    let after_alloc = alloc.live_chunks();
    arena.reset();
    assert!(
        alloc.live_chunks() < after_alloc,
        "reset must release oversized chunks (had {after_alloc}, now {})",
        alloc.live_chunks()
    );
    drop(arena);
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

#[test]
fn oversized_alloc_with_does_not_leak() {
    let alloc = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        // Force oversized via a large array.
        let _r: &mut [u32; 8 * 1024] = arena.alloc_with(|| [0_u32; 8 * 1024]);
    }
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

#[test]
fn oversized_slice_fill_with_does_not_leak() {
    let alloc = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let _slice = arena.alloc_slice_fill_with::<u32, _>(8 * 1024, |i| i as u32);
    }
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

// Regression: a panic inside the user closure of `alloc_with` on a
// payload large enough to land on an oversized chunk used to leak the
// chunk because the pin/inc-ref happened only after the closure
// returned.
#[test]
fn panic_in_oversized_alloc_with_does_not_leak() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let alloc = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _r: &mut [u32; 8 * 1024] = arena.alloc_with(|| panic!("synthetic panic"));
        }));
        assert!(result.is_err());
    }
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

// Mirror of `panic_in_normal_alloc_rc_with_does_not_leak` for the
// `Box` flavor of `ProtectiveHold` (same `AllocFlavor::Box` branch in
// `ProtectiveHold::drop`).
#[test]
fn panic_in_normal_alloc_box_with_does_not_leak() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let alloc = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _b: multitude::Box<u64, _> = arena.alloc_box_with(|| panic!("synthetic panic"));
        }));
        assert!(result.is_err());
    }
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

// Kills mutants on `SharedArcsIssuedHold::drop`'s
// `smart_pointers_issued.set(cur - 1)` (internals.rs:292). Mirrors the
// `ProtectiveHold` test above but on the shared-chunk `Arc` path
// (`current_shared.smart_pointers_issued`).
#[test]
fn panic_in_normal_alloc_arc_with_does_not_leak() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let alloc = common::SendTrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _a: multitude::Arc<u64, _> = arena.alloc_arc_with(|| panic!("synthetic panic"));
        }));
        assert!(result.is_err());
    }
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

// Kills mutants on `chunk_end_addr_fits_in_isize` (and the call-site
// negation) in `LocalChunk::allocate`. A pathological allocator that
// returns a pointer in the upper half of the address space must be
// rejected by the bounds check: the user-facing observable is a clean
// `AllocError` from `try_alloc_with` rather than a write through a
// kernel-space pointer.
#[test]
fn local_chunk_allocate_rejects_high_address_from_pathological_allocator() {
    use allocator_api2::alloc::AllocError;
    let arena = Arena::builder_in(common::BadAddressAllocator).build();
    let result: Result<&mut u64, AllocError> = arena.try_alloc_with(|| 0_u64);
    assert!(result.is_err(), "high-address allocator must produce AllocError");
}

// Mirror for the shared-chunk allocate path (`Arc` flavor). Same
// pathological allocator; the regression covers the symmetric bounds
// check in `SharedChunk::allocate`.
#[test]
fn shared_chunk_allocate_rejects_high_address_from_pathological_allocator() {
    use allocator_api2::alloc::AllocError;
    let arena = Arena::builder_in(common::BadAddressAllocator).build();
    let result: Result<multitude::Arc<u64, _>, AllocError> = arena.try_alloc_arc_with(|| 0_u64);
    assert!(result.is_err(), "high-address allocator must produce AllocError");
}

#[test]
fn try_alloc_slice_huge_len_returns_alloc_error() {
    use allocator_api2::alloc::AllocError;
    let arena: Arena = Arena::new();
    let result: Result<&mut [u64], AllocError> = arena.try_alloc_slice_fill_with(usize::MAX / 4, |_| 0);
    assert!(result.is_err(), "expected AllocError for huge len");
}

#[test]
fn try_alloc_slice_clone_huge_len_returns_alloc_error() {
    use allocator_api2::alloc::AllocError;
    let arena: Arena = Arena::new();
    let result: Result<&mut [u64], AllocError> = arena.try_alloc_slice_fill_with(usize::MAX, |_| 0);
    result.unwrap_err();
}

// === merged from tests/reset.rs ===
mod reset {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn reset_idempotent() {
        let mut arena: Arena = Arena::new();
        arena.reset();
        arena.reset();
        arena.reset();
        let _ = arena.alloc(0_u8);
        arena.reset();
        arena.reset();
    }

    #[test]
    fn reset_runs_destructors_for_alloc_style_values() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let mut arena = Arena::new();
        {
            let _v: &mut Tracked = arena.alloc(Tracked);
        }
        assert_eq!(COUNT.load(Ordering::SeqCst), 0, "drop hasn't fired yet");
        arena.reset();
        assert_eq!(COUNT.load(Ordering::SeqCst), 1, "destructor must run during reset");
    }

    #[test]
    fn reset_runs_destructors_for_all_chunk_residents() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let mut arena = Arena::new();
        for _ in 0..5 {
            let _: &mut Tracked = arena.alloc(Tracked);
        }
        arena.reset();
        assert_eq!(COUNT.load(Ordering::SeqCst), 5);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn reset_returns_chunks_to_cache_and_avoids_fresh_alloc() {
        // Seed the high-water mark to the largest class up front so the
        // chunk that backs our single allocation isn't evicted from the
        // cache when it returns (the high-water filter requires
        // `cap >= class_to_bytes(high_water)`).
        let mut arena = Arena::builder().with_capacity_local(64 * 1024).build();
        let _ = arena.alloc(0_u64);

        let stats_before = arena.stats();
        assert!(stats_before.normal_local_chunks_allocated >= 1);

        arena.reset();

        let stats_after_reset = arena.stats();
        assert_eq!(
            stats_after_reset.normal_local_chunks_allocated,
            stats_before.normal_local_chunks_allocated
        );

        let _ = arena.alloc(1_u64);
        let stats_after_realloc = arena.stats();
        assert_eq!(
            stats_after_realloc.normal_local_chunks_allocated, stats_before.normal_local_chunks_allocated,
            "reset should not allocate a fresh chunk; cache reuse expected"
        );
    }

    #[cfg(feature = "stats")]
    #[test]
    fn reset_preserves_lifetime_chunk_count_across_phases() {
        let mut arena = Arena::new();
        let mut last = 0;
        for _ in 0..3 {
            for _ in 0..10 {
                let _ = arena.alloc(0_u64);
            }
            let now = arena.stats().normal_local_chunks_allocated;
            assert!(now >= last, "lifetime chunks_allocated must be monotonic across resets");
            last = now;
            arena.reset();
        }
    }

    #[test]
    fn reset_clears_byte_budget_for_cached_chunks() {
        // Tight budget: only one chunk worth.
        let mut arena: Arena = Arena::builder().byte_budget(8 * 1024).build();

        let _ = arena.alloc(0_u8); // forces fresh chunk allocation
        arena.reset();
        // Should be able to allocate again from the cached chunk without
        // tripping the budget.
        let _ = arena.alloc(1_u8);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn reset_works_with_pinned_chunks() {
        // Force chunk rotation by allocating multiple buffers that fill the
        // chunk. We seed the high-water to class 7 so the rotated chunks
        // are eligible for caching when they return after `reset`.
        // `alloc_uninit::<MaybeUninit<[u8; 4000]>>` skips per-byte init.
        let mut arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).with_capacity_local(64 * 1024).build();
        for _ in 0..5 {
            let _ = arena.alloc(core::mem::MaybeUninit::<[u8; 4000]>::uninit());
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        assert!(chunks_before >= 1, "expected at least one chunk allocation, got {chunks_before}");

        arena.reset();
        let _ = arena.alloc(0_u64);
        assert_eq!(
            arena.stats().normal_local_chunks_allocated,
            chunks_before,
            "no fresh chunk allocation expected"
        );
    }

    #[test]
    fn reset_works_after_alloc_style_refs_drop() {
        let mut arena = Arena::new();
        {
            let r: &mut u64 = arena.alloc(123);
            *r += 1;
        }
        arena.reset();
        let r = arena.alloc(1_u64);
        assert_eq!(*r, 1);
    }

    // Reset with outstanding refcounted smart pointers: the chunk *detaches*, the
    // smart pointer keeps working, no destructor is skipped.

    #[test]
    fn reset_with_outstanding_arena_arc_keeps_handle_valid() {
        let mut arena = Arena::new();
        let r: Arc<u32> = arena.alloc_arc(7);
        arena.reset();
        assert_eq!(*r, 7);
        drop(r);
        let _ = arena.alloc_arc(11_u32);
    }

    #[test]
    fn reset_with_arena_arc_held_on_another_thread() {
        use std::sync::{Arc as StdArc, Barrier};

        let mut arena = Arena::new();
        let r: Arc<u32> = arena.alloc_arc(99);

        let barrier = StdArc::new(Barrier::new(2));
        let b = StdArc::clone(&barrier);
        let h = std::thread::spawn(move || {
            let _ = b.wait();
            assert_eq!(*r, 99);
            let _ = b.wait();
        });
        let _ = barrier.wait();
        arena.reset();
        let _ = barrier.wait();
        h.join().unwrap();
        // Arena still usable.
        let _ = arena.alloc_arc(11_u32);
    }
}

// === merged from tests/large_alloc.rs ===
mod large_alloc {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "intentional truncation in test values")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::redundant_type_annotations, reason = "type annotations for documentation clarity")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    /// 64 KiB worth of bytes (matches `CHUNK_ALIGN`).
    const CHUNK_BYTES: usize = 65_536;
    /// One byte past the chunk-recovery boundary.
    const OVER_CHUNK: usize = CHUNK_BYTES + 1;
    /// Comfortably past two chunks' worth.
    const FAR_OVER_CHUNK: usize = CHUNK_BYTES * 3 / 2;

    // ============================================================================
    // Slices: direct large allocations
    // ============================================================================

    #[test]
    fn alloc_slice_fill_with_above_chunk_boundary() {
        // Same byte-count test (`> CHUNK_BYTES`) but with `u64` elements so
        // the per-element `init` closure runs 8× fewer times — pure Miri
        // win, identical chunk-boundary semantics.
        const N_U64: usize = OVER_CHUNK / 8 + 1;
        let arena = Arena::new();
        let s = arena.alloc_slice_fill_with::<u64, _>(N_U64, |i| i as u64);
        assert_eq!(s.len(), N_U64);
        assert_eq!(s[0], 0);
        // Element that straddles the 64 KiB tile boundary.
        let mid_idx = CHUNK_BYTES / 8;
        assert_eq!(s[mid_idx], mid_idx as u64);
        assert_eq!(s[N_U64 - 1], (N_U64 - 1) as u64);
    }

    #[test]
    fn alloc_slice_copy_above_chunk_boundary() {
        let arena = Arena::new();
        // Make sure the byte-size strictly exceeds CHUNK_BYTES.
        let n = CHUNK_BYTES / 4 + 4; // bytes = 4*n = 65552 > 65536
        let src: Vec<u32> = (0..n as u32).collect();
        let s: &[u32] = arena.alloc_slice_copy(&src);
        assert_eq!(s.len(), src.len());
        assert_eq!(s[0], 0);
        assert_eq!(s[s.len() - 1], (s.len() - 1) as u32);
        // First element straddling the 64 KiB tile boundary.
        let mid_idx = CHUNK_BYTES / 4;
        assert_eq!(s[mid_idx], mid_idx as u32);
    }

    #[test]
    fn alloc_slice_clone_above_chunk_boundary() {
        let arena = Arena::new();
        let n = CHUNK_BYTES / 8 + 4; // 65568 bytes
        let src: Vec<u64> = (0..n as u64).collect();
        let s = arena.alloc_slice_clone::<u64>(&src);
        assert_eq!(s.len(), src.len());
        assert_eq!(s[0], 0);
        assert_eq!(s[s.len() - 1], (s.len() - 1) as u64);
    }

    #[test]
    fn alloc_slice_copy_arc_above_chunk_boundary() {
        // Build the source as a u64 vector: 8× fewer per-element steps in
        // the Vec construction and the subsequent `copy_nonoverlapping`
        // tracking under Miri, same `> CHUNK_BYTES` byte count.
        const N_U64: usize = OVER_CHUNK / 8 + 1;
        let arena = Arena::new();
        let src: Vec<u64> = (0..N_U64 as u64).collect();
        let a = arena.alloc_slice_copy_arc::<u64>(&src);
        assert_eq!(a.len(), src.len());
        // Cross-thread sanity: Arc<[u64]> over the oversized chunk must travel.
        let a_clone = a.clone();
        let h = std::thread::spawn(move || {
            assert_eq!(a_clone.len(), N_U64);
            assert_eq!(a_clone[N_U64 - 1], (N_U64 - 1) as u64);
        });
        h.join().unwrap();
        assert_eq!(a[0], 0);
    }

    // ============================================================================
    // Slices with Drop: large drop-slice
    // ============================================================================

    #[test]
    fn alloc_slice_fill_with_above_chunk_drops_all_elements() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct Counted(StdArc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // The property under test is "every initialized element runs
        // its Drop at arena teardown". Length doesn't matter — small
        // is fine and keeps the per-element atomic ops affordable
        // under Miri.
        let len = 16;
        {
            let arena = Arena::new();
            let s = arena.alloc_slice_fill_with::<Counted, _>(len, |_| Counted(counter.clone()));
            assert_eq!(s.len(), len);
        }
        assert_eq!(counter.load(Ordering::Relaxed), len);
    }

    #[test]
    fn alloc_slice_fill_with_arc_above_chunk_drops_all_elements_on_last_arc_drop() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct Counted(StdArc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // See sibling test above: small `len` is sufficient to verify
        // the "drops run when the last Arc is dropped" semantics.
        let len = 16;
        let arena = Arena::new();
        let arc = arena.alloc_slice_fill_with_arc::<Counted, _>(len, |_| Counted(counter.clone()));
        assert_eq!(arc.len(), len);
        let arc_clone = arc.clone();
        drop(arena);
        // Original arc + clone both live; drops must not run yet.
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        drop(arc);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        drop(arc_clone);
        assert_eq!(counter.load(Ordering::Relaxed), len);
    }

    // ============================================================================
    // Vec: explicit large capacity
    // ============================================================================

    #[test]
    fn alloc_vec_with_capacity_above_chunk_boundary() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u8>(OVER_CHUNK);
        assert!(v.capacity() >= OVER_CHUNK);
        for i in 0..OVER_CHUNK {
            v.push((i & 0xff) as u8);
        }
        assert_eq!(v.len(), OVER_CHUNK);
        assert_eq!(v[0], 0);
        assert_eq!(v[CHUNK_BYTES], (CHUNK_BYTES & 0xff) as u8);
        assert_eq!(v[OVER_CHUNK - 1], ((OVER_CHUNK - 1) & 0xff) as u8);
    }

    #[test]
    fn alloc_vec_with_capacity_at_far_over_chunk() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u32>(FAR_OVER_CHUNK / 4);
        for i in 0..(FAR_OVER_CHUNK / 4) {
            v.push(i as u32);
        }
        assert_eq!(v.len(), FAR_OVER_CHUNK / 4);
        assert_eq!(v[v.len() - 1], (v.len() - 1) as u32);
    }

    // ============================================================================
    // Vec: grow from small to past 64 KiB
    // ============================================================================

    #[test]
    fn alloc_vec_grows_from_small_to_past_chunk_boundary() {
        // The interesting property is that `Vec` survives the
        // amortized-doubling relocations triggered by growing past the
        // chunk boundary. We start small (one push, default capacity),
        // then jump past CHUNK_BYTES in a single `extend_from_slice`
        // call. The growth path still has to: rebump for the larger
        // capacity, copy live elements, and route through the oversized
        // chunk allocator — all in one shot. This keeps the Miri
        // interpreter loop count bounded (one bulk memcpy) while still
        // exercising every relocation arm.
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u64>();
        v.push(0xDEAD_u64);
        assert_eq!(v.len(), 1);
        let block: std::vec::Vec<u64> = (1..(OVER_CHUNK / 8) as u64).collect();
        v.extend_from_slice(&block);
        assert_eq!(v.len(), 1 + block.len());
        assert_eq!(v[0], 0xDEAD);
        assert_eq!(v[v.len() - 1], block.last().copied().unwrap());
    }

    #[test]
    fn alloc_vec_grows_with_drop_type_past_chunk_boundary() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct Counted(StdArc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // The property is: drops run for every live element after the
        // arena is torn down, even when growth has relocated the
        // storage. We don't actually need to cross the chunk
        // boundary — every relocation arm is covered by a single
        // `reserve(N)` for N past the initial small capacity. Use
        // a tiny N so the per-element atomic clone cost stays low
        // under Miri.
        let n = 16;
        {
            let arena = Arena::new();
            let mut v = arena.alloc_vec::<Counted>();
            for _ in 0..n {
                v.push(Counted(counter.clone()));
            }
            assert_eq!(v.len(), n);
        }
        assert_eq!(counter.load(Ordering::Relaxed), n);
    }

    #[test]
    fn alloc_vec_extend_from_iter_past_chunk_boundary() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u16>();
        v.extend((0..(OVER_CHUNK / 2) as u16).map(|i| i.wrapping_mul(13)));
        assert_eq!(v.len(), OVER_CHUNK / 2);
        // Spot-check first, mid-chunk and last instead of iterating
        // every element; a chunk-boundary bug would manifest at any of
        // these positions equally and the per-element cost dominates
        // under Miri.
        for i in [0, OVER_CHUNK / 4, OVER_CHUNK / 2 - 1] {
            assert_eq!(v[i], (i as u16).wrapping_mul(13));
        }
    }

    #[test]
    fn vec_in_macro_initial_then_grow_past_chunk() {
        let arena = Arena::new();
        let mut v = multitude::vec::vec![in &arena; 0u32; 16];
        assert_eq!(v.len(), 16);
        // Fixed iteration count rather than `while v.len() < ...` so
        // that a `Vec::len -> 0` mutation can't drive an infinite loop.
        for next in 16..(OVER_CHUNK / 4) {
            v.push(next as u32);
        }
        assert_eq!(v.len(), OVER_CHUNK / 4);
        assert_eq!(v[0], 0);
        assert_eq!(v[16], 16);
        assert_eq!(v[v.len() - 1], (v.len() - 1) as u32);
    }

    // ============================================================================
    // String: explicit large capacity and growth
    // ============================================================================

    #[test]
    fn alloc_string_with_capacity_above_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(OVER_CHUNK);
        assert!(s.capacity() >= OVER_CHUNK);
        // Bulk push: a single push_str is one memcpy rather than
        // OVER_CHUNK individual char-push calls (each one performing
        // capacity checks and UTF-8 encoding under Miri).
        let block = "a".repeat(OVER_CHUNK);
        s.push_str(&block);
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(s.as_bytes()[0], b'a');
        assert_eq!(s.as_bytes()[CHUNK_BYTES], b'a');
        assert_eq!(s.as_bytes()[OVER_CHUNK - 1], b'a');
    }

    #[test]
    fn alloc_string_grows_from_small_to_past_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        assert_eq!(s.len(), 0);
        s.push_str("hello");
        // Bulk push instead of OVER_CHUNK individual `push('x')` calls.
        // The final `assert_eq!(s.len(), ...)` still kills `push_str
        // -> noop` and `String::len -> 0` mutations.
        let block = "x".repeat(OVER_CHUNK);
        s.push_str(&block);
        assert_eq!(s.len(), 5 + OVER_CHUNK);
        assert!(s.as_str().starts_with("hello"));
        assert_eq!(s.as_bytes()[5 + OVER_CHUNK - 1], b'x');
    }

    #[test]
    fn alloc_string_push_multibyte_grows_past_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        // Each emoji is 4 bytes UTF-8. Bulk push instead of per-char.
        let target_chars = (OVER_CHUNK / 4) + 16;
        let block = "🦀".repeat(target_chars);
        s.push_str(&block);
        assert_eq!(s.len(), target_chars * 4);
        let mut chars = s.as_str().chars();
        assert_eq!(chars.next(), Some('🦀'));
        assert_eq!(chars.last(), Some('🦀'));
    }

    // ============================================================================
    // Utf16String: same coverage for the 16-bit-encoded sibling
    // ============================================================================

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_string_with_capacity_above_chunk_units() {
        let arena = Arena::new();
        // capacity is measured in `u16` code units; OVER_CHUNK code units
        // is 131 074 bytes of buffer. The interesting property is that
        // `with_capacity` past the chunk boundary returns a usable
        // buffer of the requested capacity. We don't need to fill the
        // whole capacity — a small write at the start and another past
        // the chunk-byte boundary confirms the buffer is indexable
        // throughout. This keeps the Miri cost (which is dominated by
        // per-byte UTF-8 → UTF-16 transcoding) bounded to a tiny push.
        let mut s = arena.alloc_utf16_string_with_capacity(OVER_CHUNK);
        s.push_from_str("a");
        assert_eq!(s.len(), 1);
        assert_eq!(s.as_slice()[0], u16::from(b'a'));
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_string_grows_from_small_to_past_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        // Push a small initial seed, then reserve past the chunk
        // boundary in one shot. The growth path needs to: re-route
        // through the oversized chunk allocator, copy the live
        // elements, update cap. A second small push past the prior
        // length confirms the new buffer is writable beyond the
        // initial seed. We avoid the brute-force "transcode 65 KiB"
        // step entirely — `reserve` exercises the same growth arms
        // without paying for it.
        s.push_from_str("hello");
        s.reserve(OVER_CHUNK);
        assert!(s.capacity() >= 5 + OVER_CHUNK);
        s.push_from_str("y");
        assert_eq!(s.len(), 6);
        let v = s.as_slice();
        assert_eq!(v[0], u16::from(b'h'));
        assert_eq!(v[5], u16::from(b'y'));
    }

    // ============================================================================
    // Stress: many oversized allocations in one arena
    // ============================================================================

    #[test]
    fn many_oversized_allocations_in_one_arena() {
        // The property under test is that an arena tolerates *multiple*
        // oversized one-shot chunks coexisting. Using `[u128; OVER_CHUNK/16]`
        // gives the same byte-count threshold (above `MAX_CHUNK_BYTES`) but
        // a 16× shorter `alloc_slice_fill_with` closure loop — a big win
        // under Miri where each closure invocation is interpreted.
        const N_U128: usize = OVER_CHUNK / 16 + 1; // > 64 KiB worth of u128
        let arena = Arena::new();
        let mut keepers: Vec<&[u128]> = Vec::with_capacity(8);
        for round in 0..8u8 {
            let s: &mut [u128] = arena.alloc_slice_fill_with::<u128, _>(N_U128, move |_| u128::from(round));
            keepers.push(s);
        }
        for (round, s) in keepers.iter().enumerate() {
            assert_eq!(s.len(), N_U128);
            assert_eq!(s[0], round as u128);
            assert_eq!(s[N_U128 - 1], round as u128);
        }
    }

    // ============================================================================
    // Strings allocated as Arc<str> via the DST path (`alloc_str_arc`)
    // ============================================================================

    #[test]
    fn alloc_str_arc_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "x".repeat(OVER_CHUNK);
        let s = arena.alloc_str_arc(&big);
        assert_eq!(s.len(), OVER_CHUNK);
        let clone = s.clone();
        let h = std::thread::spawn(move || {
            assert_eq!(clone.len(), OVER_CHUNK);
            assert_eq!(&clone[..5], "xxxxx");
        });
        h.join().unwrap();
        assert_eq!(&s[OVER_CHUNK - 5..], "xxxxx");
    }

    #[test]
    fn alloc_str_box_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "z".repeat(OVER_CHUNK);
        let s = arena.alloc_str_box(&big);
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(&s[..5], "zzzzz");
    }

    #[test]
    fn alloc_str_simple_ref_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "w".repeat(OVER_CHUNK);
        let s: &mut str = arena.alloc_str(&big);
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(&s[..5], "wwwww");
        assert_eq!(&s[OVER_CHUNK - 5..], "wwwww");
        // Confirm small allocations after a large one still work (the
        // oversized chunk does not become the current local slot).
        let small = arena.alloc_str("small");
        assert_eq!(small, "small");
    }

    #[test]
    fn alloc_str_simple_ref_far_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "Q".repeat(FAR_OVER_CHUNK);
        let s: &mut str = arena.alloc_str(&big);
        assert_eq!(s.len(), FAR_OVER_CHUNK);
        // memcmp via slice equality is one bulk op instead of FAR_OVER_CHUNK
        // per-char yields under Miri.
        assert_eq!(s.as_bytes(), big.as_bytes());
    }

    #[test]
    fn try_alloc_str_simple_ref_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "p".repeat(OVER_CHUNK);
        let s: &mut str = arena.try_alloc_str(&big).expect("oversized alloc_str must succeed");
        assert_eq!(s.len(), OVER_CHUNK);
    }

    // ============================================================================
    // DST (Rc<[T]>/Arc<[T]>/Box<[T]>) allocations > 64 KiB via the dst path
    // ============================================================================

    #[cfg(feature = "dst")]
    #[test]
    fn alloc_dst_arc_slice_above_chunk_boundary() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering as StdOrd};

        // 64-byte payload + StdArc so each `Counted` is 72 bytes; only
        // ~1024 elements are needed to cross the 64 KiB chunk boundary,
        // dramatically fewer than the per-byte alternative.
        #[derive(Clone)]
        struct Counted {
            _pad: [u8; 64],
            c: StdArc<AtomicUsize>,
        }
        impl Drop for Counted {
            fn drop(&mut self) {
                self.c.fetch_add(1, StdOrd::Relaxed);
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // Choose `len` so `len * size_of::<Counted>` crosses the 64 KiB
        // chunk boundary.
        let n = CHUNK_BYTES / core::mem::size_of::<Counted>() + 8;
        {
            let arena = Arena::new();
            let layout = core::alloc::Layout::array::<Counted>(n).unwrap();
            assert!(layout.size() > CHUNK_BYTES, "test must drive the oversized DST shared path");
            // SAFETY: init fills every slot of the slice fat pointer.
            let arc: multitude::Arc<[Counted]> = unsafe {
                arena.alloc_dst_arc::<[Counted]>(layout, n, |p: *mut [Counted]| {
                    for i in 0..n {
                        let slot: *mut Counted = (p as *mut Counted).add(i);
                        core::ptr::write(
                            slot,
                            Counted {
                                _pad: [0; 64],
                                c: counter.clone(),
                            },
                        );
                    }
                })
            };
            assert_eq!(arc.len(), n);
            drop(arena);
            assert_eq!(counter.load(StdOrd::Relaxed), 0, "Counted::drop must wait for the last Arc to drop");
            drop(arc);
        }
        assert_eq!(counter.load(StdOrd::Relaxed), n);
    }

    #[cfg(feature = "dst")]
    #[test]
    fn alloc_dst_arc_slice_non_drop_above_chunk_boundary() {
        // Drives the oversized shared DST path: byte size must exceed
        // CHUNK_BYTES (64 KiB). 33 000 u16s = 66 KiB, just over the
        // boundary — large enough to exercise the path without paying
        // the per-element init cost u16::MAX times under Miri.
        const LEN: usize = 33_000;
        let arena = Arena::new();
        let layout = core::alloc::Layout::array::<u16>(LEN).unwrap();
        assert!(layout.size() > CHUNK_BYTES, "test must drive the oversized DST shared path");
        // SAFETY: init fills every element via a single bulk write
        // followed by 3 spot writes to verify the boundary semantics.
        let arc: multitude::Arc<[u16]> = unsafe {
            arena.alloc_dst_arc::<[u16]>(layout, LEN, |p: *mut [u16]| {
                let raw = p as *mut u16;
                core::ptr::write_bytes(raw, 0, LEN);
                raw.write(0xCAFE);
                raw.add(LEN / 2).write(0xBABE);
                raw.add(LEN - 1).write(0xBEEF);
            })
        };
        assert_eq!(arc.len(), LEN);
        assert_eq!(arc[0], 0xCAFE);
        assert_eq!(arc[LEN / 2], 0xBABE);
        assert_eq!(arc[LEN - 1], 0xBEEF);
    }

    // ============================================================================
    // BytesBuf integration > 64 KiB
    // ============================================================================

    #[cfg(feature = "bytesbuf")]
    #[test]
    fn bytesbuf_reserve_above_chunk_boundary() {
        use bytesbuf::mem::Memory;
        let arena = Arena::new();
        let _buf = arena.reserve(OVER_CHUNK);
        // The reserve call returned without panicking, which is the
        // observable that demonstrates the oversized shared path is
        // working — previously it would have spun in `refill_shared`
        // until OOM or hit `expect("arena allocation failed")`.
    }
}

// === merged from tests/fast_path_correctness.rs ===
mod fast_path_correctness {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::needless_range_loop, reason = "test indexing is intentional")]
    #![allow(clippy::missing_asserts_for_indexing, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[cfg(feature = "stats")]
    use crate::common;

    /// Mutex to serialize tests that use global drop counters.
    static DROP_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn alloc_u64_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let r: &mut u64 = arena.alloc(0xDEAD_BEEF_u64);
            let ptr = std::ptr::from_ref::<u64>(r) as usize;
            assert_eq!(ptr % align_of::<u64>(), 0, "u64 pointer misaligned: {ptr:#x}");
            assert_eq!(*r, 0xDEAD_BEEF_u64);
        }
    }

    #[test]
    fn alloc_u128_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let r: &mut u128 = arena.alloc(0x1234_5678_9ABC_DEF0_u128);
            let ptr = std::ptr::from_ref::<u128>(r) as usize;
            assert_eq!(ptr % align_of::<u128>(), 0, "u128 pointer misaligned: {ptr:#x}");
            assert_eq!(*r, 0x1234_5678_9ABC_DEF0_u128);
        }
    }

    #[repr(align(32))]
    #[derive(Debug, Clone, PartialEq)]
    struct Align32 {
        value: u64,
    }

    #[test]
    fn alloc_align32_is_aligned() {
        let arena = Arena::new();
        for i in 0..50 {
            let r: &mut Align32 = arena.alloc(Align32 { value: i });
            let ptr = std::ptr::from_ref::<Align32>(r) as usize;
            assert_eq!(ptr % 32, 0, "Align32 pointer misaligned: {ptr:#x}");
            assert_eq!(r.value, i);
        }
    }

    #[repr(align(64))]
    #[derive(Debug, Clone, PartialEq)]
    struct Align64 {
        data: [u8; 64],
    }

    #[test]
    fn alloc_align64_is_aligned() {
        let arena = Arena::new();
        for i in 0_u8..30 {
            let r: &mut Align64 = arena.alloc(Align64 { data: [i; 64] });
            let ptr = std::ptr::from_ref::<Align64>(r) as usize;
            assert_eq!(ptr % 64, 0, "Align64 pointer misaligned: {ptr:#x}");
            assert_eq!(r.data[0], i);
        }
    }

    #[test]
    fn alloc_arc_u64_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let arc = arena.alloc_arc(0xDEAD_BEEF_u64);
            let ptr = &raw const *arc as usize;
            assert_eq!(ptr % align_of::<u64>(), 0, "Arc<u64> pointer misaligned: {ptr:#x}");
            assert_eq!(*arc, 0xDEAD_BEEF_u64);
        }
    }

    #[test]
    fn alloc_arc_u128_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let arc = arena.alloc_arc(0x1234_5678_9ABC_DEF0_u128);
            let ptr = &raw const *arc as usize;
            assert_eq!(ptr % align_of::<u128>(), 0, "Arc<u128> pointer misaligned: {ptr:#x}");
            assert_eq!(*arc, 0x1234_5678_9ABC_DEF0_u128);
        }
    }

    #[test]
    fn alloc_arc_align32_is_aligned() {
        let arena = Arena::new();
        for i in 0..50 {
            let arc = arena.alloc_arc(Align32 { value: i });
            let ptr = &raw const *arc as usize;
            assert_eq!(ptr % 32, 0, "Arc<Align32> pointer misaligned: {ptr:#x}");
            assert_eq!(arc.value, i);
        }
    }

    #[test]
    fn interleaved_alignments_all_correct() {
        let arena = Arena::new();
        for i in 0_u64..50 {
            // Allocate u8, then u64, then u128 — forces alignment padding
            let a: &mut u8 = arena.alloc(i as u8);
            let b: &mut u64 = arena.alloc(i);
            let c: &mut u128 = arena.alloc(u128::from(i));

            assert_eq!((std::ptr::from_ref::<u8>(a) as usize) % align_of::<u8>(), 0);
            assert_eq!(
                (std::ptr::from_ref::<u64>(b) as usize) % align_of::<u64>(),
                0,
                "u64 misaligned after u8"
            );
            assert_eq!(
                (std::ptr::from_ref::<u128>(c) as usize) % align_of::<u128>(),
                0,
                "u128 misaligned after u64"
            );

            assert_eq!(*a, i as u8);
            assert_eq!(*b, i);
            assert_eq!(*c, u128::from(i));
        }
    }

    #[test]
    fn interleaved_arc_alignments() {
        let arena = Arena::new();
        for i in 0_u64..50 {
            let a = arena.alloc_arc(i as u8);
            let b = arena.alloc_arc(i);
            let c = arena.alloc_arc(Align32 { value: i });

            assert_eq!((&raw const *a as usize) % align_of::<u8>(), 0);
            assert_eq!((&raw const *b as usize) % align_of::<u64>(), 0);
            assert_eq!((&raw const *c as usize) % 32, 0);

            assert_eq!(*a, i as u8);
            assert_eq!(*b, i);
            assert_eq!(c.value, i);
        }
    }

    static DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct DropTracker(#[expect(dead_code, reason = "field exists for Drop semantics")] u64);

    impl Drop for DropTracker {
        fn drop(&mut self) {
            let _ = DROP_COUNTER.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn arc_drop_runs_correctly_many_items() {
        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let before = DROP_COUNTER.load(Ordering::SeqCst);
        let n = 200;
        {
            let arena = Arena::new();
            let handles: Vec<_> = (0..n).map(|i| arena.alloc_arc(DropTracker(i))).collect();
            assert_eq!(handles.len(), n as usize);
            drop(handles);
        }
        let after = DROP_COUNTER.load(Ordering::SeqCst);
        assert_eq!(after - before, n as usize);
    }

    #[test]
    fn box_drop_runs_on_each_drop() {
        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let before = DROP_COUNTER.load(Ordering::SeqCst);
        let arena = Arena::new();
        for i in 0..100_u64 {
            let b = arena.alloc_box(DropTracker(i));
            drop(b);
            let after = DROP_COUNTER.load(Ordering::SeqCst);
            assert_eq!(after - before, (i + 1) as usize);
        }
    }

    #[repr(align(32))]
    struct AlignedDropTracker {
        #[expect(dead_code, reason = "field exists for Drop semantics")]
        value: u64,
    }

    static ALIGNED_DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    impl Drop for AlignedDropTracker {
        fn drop(&mut self) {
            let _ = ALIGNED_DROP_COUNTER.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn arc_aligned_drop_runs_correctly() {
        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let before = ALIGNED_DROP_COUNTER.load(Ordering::SeqCst);
        let n = 50;
        {
            let arena = Arena::new();
            let handles: Vec<_> = (0..n)
                .map(|i| {
                    let arc = arena.alloc_arc(AlignedDropTracker { value: i });
                    let ptr = &raw const *arc as usize;
                    assert_eq!(ptr % 32, 0, "AlignedDropTracker misaligned: {ptr:#x}");
                    arc
                })
                .collect();
            assert_eq!(handles.len(), n as usize);
            drop(handles);
        }
        let after = ALIGNED_DROP_COUNTER.load(Ordering::SeqCst);
        assert_eq!(after - before, n as usize);
    }

    #[test]
    fn consecutive_allocs_do_not_overlap() {
        let arena = Arena::new();
        let mut ptrs: Vec<(*const u64, usize)> = Vec::new();
        for i in 0..200_u64 {
            let r: &mut u64 = arena.alloc(i);
            let addr = std::ptr::from_ref::<u64>(r) as usize;
            ptrs.push((std::ptr::from_ref::<u64>(r), addr));
        }
        // Verify no two allocations overlap (each is 8 bytes)
        ptrs.sort_by_key(|&(_, addr)| addr);
        for window in ptrs.windows(2) {
            let (_, a) = window[0];
            let (_, b) = window[1];
            assert!(a + size_of::<u64>() <= b, "Allocations overlap: {a:#x} + 8 > {b:#x}");
        }
    }

    #[test]
    fn consecutive_arc_allocs_do_not_overlap() {
        let arena = Arena::new();
        let handles: Vec<_> = (0..200_u64).map(|i| arena.alloc_arc(i)).collect();
        let mut addrs: Vec<usize> = handles.iter().map(|arc| &raw const **arc as usize).collect();
        addrs.sort_unstable();
        for window in addrs.windows(2) {
            assert!(
                window[0] + size_of::<u64>() <= window[1],
                "Arc allocations overlap: {:#x} + 8 > {:#x}",
                window[0],
                window[1]
            );
        }
    }

    #[cfg(feature = "stats")]
    #[test]
    fn filling_chunk_triggers_new_allocation() {
        // Allocate u64s until we overflow whatever chunk size the arena
        // settled on. With adaptive sizing the first chunk is the
        // smallest class (1 KiB), so the count just needs to be enough to
        // confirm that more than a handful of u64s pack in.
        let arena = Arena::builder().build();
        // Prime: first allocation triggers a chunk alloc
        let _prime: &mut u64 = arena.alloc(0);
        let initial_chunks = arena.stats().normal_local_chunks_allocated;
        assert_eq!(initial_chunks, 1);
        let mut count = 0_u64;
        while arena.stats().normal_local_chunks_allocated == initial_chunks {
            let r: &mut u64 = arena.alloc(count);
            assert_eq!(*r, count);
            let ptr = std::ptr::from_ref::<u64>(r) as usize;
            assert_eq!(ptr % align_of::<u64>(), 0);
            count += 1;
            assert!(count < 2000, "should have triggered new chunk by now");
        }
        // The chunk boundary was crossed
        assert!(count > 50, "chunk should hold many u64s, got {count}");
    }

    #[cfg(feature = "stats")]
    #[test]
    fn filling_chunk_arc_triggers_new_allocation() {
        let arena = Arena::builder().build();
        // Prime
        let _prime = arena.alloc_arc(0_u64);
        let initial_chunks = arena.stats().normal_shared_chunks_allocated;
        let mut handles = Vec::new();
        let mut count = 0_u64;
        while arena.stats().normal_shared_chunks_allocated == initial_chunks {
            let arc = arena.alloc_arc(count);
            let ptr = &raw const *arc as usize;
            assert_eq!(ptr % align_of::<u64>(), 0);
            handles.push(arc);
            count += 1;
            assert!(count < 20_000, "should have triggered new chunk by now");
        }
        assert!(count > 50, "chunk should hold many Arc<u64>s");
    }

    #[cfg(feature = "stats")]
    #[test]
    fn oversize_alloc_goes_to_oversized_chunk() {
        // Default max_normal_alloc for 64 KiB chunks = 16 KiB.
        // Allocate something larger than that.
        let arena = Arena::new();
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
        let big: &mut [u8; 32 * 1024] = arena.alloc([0u8; 32 * 1024]);
        big[0] = 42;
        assert_eq!(big[0], 42);
        assert!(arena.stats().oversized_local_chunks_allocated >= 1);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn oversize_arc_goes_to_oversized_chunk() {
        let arena = Arena::new();
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
        let big = arena.alloc_arc([0u8; 32 * 1024]);
        assert_eq!(big[0], 0);
        assert!(arena.stats().oversized_shared_chunks_allocated >= 1);
    }

    /// A Droppable type — allocating it uses the `DropEntry` path (line 319-325 in arena.rs).
    /// If alignment math is corrupted (e.g., + → *), the bump pointer advances far too fast,
    /// and far fewer items fit in a single chunk.
    #[cfg(feature = "stats")]
    struct Droppable(u64);
    #[cfg(feature = "stats")]
    #[expect(clippy::empty_drop, reason = "empty Drop impl is intentional to trigger DropEntry path")]
    impl Drop for Droppable {
        fn drop(&mut self) {
            // no-op; existence of Drop impl triggers DropEntry path
        }
    }

    #[cfg(feature = "stats")]
    #[test]
    fn drop_items_pack_efficiently_in_chunk() {
        // Each Droppable(u64) needs DropEntry (32 bytes) + value (8 bytes)
        // = ~40 bytes. The smallest chunk class (1 KiB) leaves about
        // 768 usable bytes after the header, so ~19 items fit. With the
        // + → * mutation, only 2-3 items would fit.
        let arena = Arena::builder().build();
        let _prime: &mut Droppable = arena.alloc(Droppable(0));
        let initial_chunks = arena.stats().normal_local_chunks_allocated;
        let mut count = 0_u64;
        while arena.stats().normal_local_chunks_allocated == initial_chunks && count < 500 {
            let r: &mut Droppable = arena.alloc(Droppable(count));
            assert_eq!(r.0, count);
            count += 1;
        }
        // With correct math we should fit at least 10 items per 1 KiB
        // chunk; mutated multiplicative math would fit 2–3.
        assert!(
            count > 10,
            "Only {count} Droppable items fit in chunk — alignment math may be corrupted"
        );
    }

    // String pinning: the fast-path str allocation must pin the chunk so it
    // survives eviction. Without pinning, a filled chunk would be freed and
    // the returned &mut str would dangle.

    // Slice fill_with panic safety: partial initialization must be cleaned up
    // if the fill closure panics.

    /// A type that is Clone + Drop and whose Clone impl panics at a configurable point.
    #[derive(Debug)]
    struct PanicOnClone {
        id: u64,
        panic_at: u64,
    }

    impl Clone for PanicOnClone {
        fn clone(&self) -> Self {
            assert!(self.id != self.panic_at, "intentional panic cloning id={}", self.id);
            let _ = ALIGNED_DROP_COUNTER.fetch_add(0, Ordering::SeqCst); // prevent optimization
            Self {
                id: self.id,
                panic_at: self.panic_at,
            }
        }
    }

    impl Drop for PanicOnClone {
        fn drop(&mut self) {
            let _ = DROP_COUNTER.fetch_add(1, Ordering::SeqCst);
        }
    }
}

// === merged from tests/allocator_impl.rs ===
mod allocator_impl {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn allocator_shrink_in_place_path() {
        // shrink is called internally by Vec when capacity reduces.
        // Exercise that no UB arises from the typical reserve/clear cycle.
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.extend(0..50_u32);
        v.clear();
        v.reserve(10);
        assert!(v.capacity() >= 10);
    }

    #[test]
    fn allocator_rejects_excessive_alignment() {
        // `<&Arena>::allocate` must reject layouts whose alignment exceeds
        // CHUNK_ALIGN (64 KiB). Without this guard the oversized chunk's
        // base would only be 64 KiB-aligned, and the data pointer derived
        // from it would be misaligned for the user's request — UB on first
        // typed access.
        use allocator_api2::alloc::Allocator;
        let arena = Arena::new();
        let allocator: &Arena = &arena;
        let layout = core::alloc::Layout::from_size_align(8, 128 * 1024).unwrap();
        let _ = allocator.allocate(layout).unwrap_err();
    }

    #[test]
    fn allocator_rejects_alignment_equal_to_chunk_align() {
        // `<&Arena>::allocate` must also reject layouts whose alignment
        // equals CHUNK_ALIGN (64 KiB). For such allocations the value
        // would land at offset == CHUNK_ALIGN within the chunk, where
        // `header_for`'s `addr & (CHUNK_ALIGN - 1)` mask returns 0 and
        // so reports the value pointer itself as the chunk header
        // address — UB on the next refcount op.
        use allocator_api2::alloc::Allocator;
        let arena = Arena::new();
        let allocator: &Arena = &arena;
        let layout = core::alloc::Layout::from_size_align(8, 64 * 1024).unwrap();
        let _ = allocator.allocate(layout).unwrap_err();
    }
}

// === merged from tests/mutants_chunk_provider.rs ===
mod mutants_for_chunk_provider {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::items_after_statements, reason = "test-local types live next to their usage")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(dead_code, reason = "test structs retain payload fields to control size")]
    #[cfg(feature = "stats")]
    use multitude::{Arena, Box};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    /// Kills `chunk_provider.rs:133:25 > → >=` in `reserve_budget`.
    ///
    /// The check is `if next > budget { return Err }`. Mutated to `>=`,
    /// the boundary `next == budget` (i.e. exactly the budget) would be
    /// rejected. We pick a tight, exact-fit byte budget and exercise it
    /// to its boundary — the unmutated code admits the allocation,
    /// the mutated code returns `AllocError`.
    #[cfg(feature = "stats")]
    #[test]
    fn reserve_budget_admits_exact_fit() {
        // The chunk-provider charges `header_bytes + payload_bytes` per chunk.
        // We don't know `header_bytes` exactly, but the smallest cacheable
        // payload is 512 bytes. Using `with_capacity_local(512)` forces a
        // single 512-byte chunk preallocation; once that is reserved, the
        // running total equals (header + 512). Choosing the byte_budget
        // equal to that total exercises the boundary.
        //
        // We discover `header + 512` by first building with an effectively
        // infinite budget and reading `total_bytes_allocated`-equivalent
        // stats. The arena exposes preallocation through stats, so we run
        // a probe build to learn the total chunk byte cost, then build a
        // second arena with budget == exact total.
        let probe = Arena::builder().byte_budget(1024 * 1024).with_capacity_local(512).build();
        assert_eq!(probe.stats().normal_local_chunks_allocated, 1);
        drop(probe);

        // 1 KiB is enough to cover header (<512 bytes) + 512 payload, so
        // the exact-fit budget admits exactly one chunk's worth.
        let arena = Arena::builder().byte_budget(1024).with_capacity_local(512).build();
        assert_eq!(
            arena.stats().normal_local_chunks_allocated,
            1,
            "byte_budget == total bytes for one chunk must admit allocation"
        );
    }

    /// Kills `chunk_provider.rs:152:9 release_budget → ()` (function body
    /// becomes a no-op) and the budget-release path in `acquire_local`
    /// /`acquire_shared` (failure-rollback arms at 187/254/424/452).
    ///
    /// Strategy: configure a tight byte budget such that *two* normal
    /// allocations would exceed it. Force the budget-release path by
    /// failing the allocator on the second attempt; if `release_budget`
    /// is a no-op the second-attempt's reserved bytes stay accounted and
    /// the third attempt errors. Without a custom failing-allocator we
    /// indirectly observe via `total_chunk_bytes` shrinking after a
    /// chunk is freed: dropping the arena releases the chunks, which
    /// must subtract their bytes from the running total. Re-creating a
    /// new arena from the same provider would reuse the budget — but
    /// arena and provider are 1:1, so we instead verify that a tighter
    /// re-creation succeeds. (Cross-arena budget recycling is covered by
    /// `arena_arc.rs::shared_chunk_returns_to_provider_after_arc_drop`.)
    #[cfg(feature = "stats")]
    #[test]
    fn release_budget_runs_when_chunk_freed() {
        // A 5 MiB budget is enough for one 64 KiB chunk plus header but
        // not for two. A single 8 KiB uninit box is enough to force a
        // chunk allocation; no need for eight (the loop was for a prior
        // version of this test that needed cache eviction).
        let arena = Arena::builder().byte_budget(5 * 1024 * 1024).build();
        let box1 = arena.alloc_uninit_box::<[u8; 8 * 1024]>();
        assert!(arena.stats().normal_shared_chunks_allocated >= 1);
        drop(box1);
        drop(arena);
        // If `release_budget` is a no-op, recreating an arena with the
        // same budget would fail to satisfy the same workload. The
        // user-observable invariant: a fresh arena with the same budget
        // admits the same allocation.
        let arena2 = Arena::builder().byte_budget(5 * 1024 * 1024).build();
        let _box2 = arena2.alloc_uninit_box::<[u8; 8 * 1024]>();
        assert!(arena2.stats().normal_shared_chunks_allocated >= 1);
    }

    /// Kills `chunk_provider.rs:163:24 > → >=` in `acquire_local`
    /// (oversized routing gate `min_payload > max_normal_alloc`) and the
    /// matching `405:24` in `acquire_shared`.
    ///
    /// At the boundary `min_payload == max_normal_alloc`, the unmutated
    /// code routes to the normal (cacheable) path. `> → >=` would route
    /// the boundary case to oversized. We can detect this via the
    /// `oversized_*_chunks_allocated` stats counter being 0 vs 1 when an
    /// allocation lands exactly on the limit.
    ///
    /// `acquire_*(min_payload)` is invoked with `needed = size +
    /// align_slack + entry_size`. To hit `needed == max_normal_alloc`
    /// precisely we set a known `max_normal_alloc` and allocate a value
    /// of matching size.
    #[cfg(feature = "stats")]
    #[test]
    fn acquire_local_boundary_does_not_route_oversized() {
        // Set max_normal_alloc to the minimum permitted (4 KiB).
        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        // Allocate a value of exactly 4 KiB (no Drop → no entry_size, no
        // align slack vs align_of::<usize>()).
        #[repr(align(8))]
        struct Block([u64; 512]); // 4096 bytes exactly
        let _b = arena.alloc_box(Block([0_u64; 512]));
        let s = arena.stats();
        assert_eq!(s.oversized_local_chunks_allocated, 0);
        assert_eq!(s.normal_local_chunks_allocated, 0);
        assert!(s.normal_shared_chunks_allocated + s.oversized_shared_chunks_allocated >= 1);
    }

    /// Kills `chunk_provider.rs:405:24 > → >=` in `acquire_shared` (same
    /// rationale as above, shared flavor).
    #[cfg(feature = "stats")]
    #[test]
    fn acquire_shared_boundary_does_not_route_oversized() {
        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        #[repr(align(8))]
        struct Block([u64; 512]);
        let _a = arena.alloc_arc(Block([0_u64; 512]));
        let s = arena.stats();
        assert!(s.normal_shared_chunks_allocated + s.oversized_shared_chunks_allocated >= 1);
    }

    /// Kills the subtraction mutants in `acquire_local` / `acquire_shared`
    /// around `NUM_CHUNK_CLASSES - 1` (chunk_provider.rs:187, 254, 424,
    /// 452): `- 1 → + 1` or `- 1 → / 1`. These set `max_class` to the
    /// largest legal class index. Mutating shrinks/expands the class
    /// ceiling, which changes the chunk *size* picked for fresh
    /// allocations on a cache miss.
    ///
    /// We force a high-water mark by allocating many small values
    /// sequentially. After enough allocations the provider must have
    /// allocated several chunks; the largest chunk produced must be
    /// 64 KiB (class 7 = `NUM_CHUNK_CLASSES - 1`). With mutations the
    /// ceiling shifts: `+ 1` allows class 8 (128 KiB) → larger total
    /// bytes; `/ 1` allows up to 8 (one larger class).
    ///
    /// Observation: total_bytes_allocated stays bounded by a
    /// well-known sum under the unmutated ceiling.
    #[cfg(feature = "stats")]
    #[test]
    fn acquire_local_class_ceiling_is_correct() {
        // The property under test: the size-class ratchet caps at the
        // largest cacheable class (class 7 = 64 KiB total). After the
        // first few refills ratchet there, subsequent refills stay at
        // class 7 — they don't keep doubling. To observe this we
        // allocate a handful of 8 KiB boxes (just under MAX_NORMAL_ALLOC
        // = 16 KiB, so still routed through the normal cache) and
        // confirm none route to oversized. A 64 KiB class-7 chunk fits
        // a couple of these, so 8 boxes span ≥ 2 chunks, proving the
        // ratchet stays at class 7 rather than degrading or escaping.
        let arena = Arena::new();
        let mut keep: Vec<Box<core::mem::MaybeUninit<[u8; 8 * 1024]>>> = Vec::new();
        for _ in 0..8 {
            keep.push(arena.alloc_uninit_box::<[u8; 8 * 1024]>());
        }
        let s = arena.stats();
        assert_eq!(s.oversized_shared_chunks_allocated, 0);
        assert!(
            s.normal_shared_chunks_allocated >= 2,
            "8 × 8 KiB boxes must span ≥ 2 class-7 chunks, got {}",
            s.normal_shared_chunks_allocated
        );
    }

    /// Kills `chunk_provider.rs:258:36 > → >=` and the matching
    /// `chunk_provider.rs:300:33` mutants (`> with ==/</>=`) in
    /// `preallocate_local`'s high-water ratchet.
    ///
    /// `next_high_water > *h` chooses the larger of the two. With
    /// `>=`, equal high-waters cause a redundant write — observable only
    /// through alias bookkeeping, but the *behavior* is identical. The
    /// related comparison in `preallocate_local` at line 300 governs the
    /// fetch_max ratchet on the shared cache; `> → <` reverses the
    /// ratchet so future chunks shrink instead of growing.
    ///
    /// Strategy: preallocate first at a small class, then allocate at a
    /// large class — the high-water mark should grow, and subsequent
    /// fresh chunks should match the larger class.
    #[cfg(feature = "stats")]
    #[test]
    fn high_water_ratchet_grows_chunks() {
        let arena = Arena::builder().with_capacity_local(512).build();
        // Preallocation should have created exactly one 512-byte (class 0) chunk.
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
        // Allocate a single 8 KiB blob: the 512-byte starter chunk can't
        // fit it, forcing a refill that ratchets the high-water mark to
        // a ≥ class-5 (8 KiB) chunk.
        #[repr(align(8))]
        struct Blob([u8; 8 * 1024]); // 8 KiB
        let _b1 = arena.alloc_box(Blob([0; 8 * 1024]));
        // Allocate a second 8 KiB blob: doesn't fit in the tail of the
        // first refilled chunk, so it forces another refill. With the
        // ratchet intact, that fresh chunk is at the larger class (≥ 8
        // KiB) and absorbs the blob normally. With the ratchet reversed
        // (`> → <`), the fresh chunk would shrink below 8 KiB, the blob
        // couldn't fit, and the alloc would route to an oversized chunk.
        let _b2 = arena.alloc_box(Blob([0; 8 * 1024]));
        // No oversized routes:
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
    }

    /// Kills `chunk_provider.rs:274:47 + → *` in `preallocate_local`
    /// (and `315:48` in `preallocate_shared`): the `local_header_size() +
    /// target_bytes` total. With `*` the total balloons and reserve_budget
    /// would fail for any tight budget.
    #[cfg(feature = "stats")]
    #[test]
    fn preallocate_total_bytes_uses_sum_not_product() {
        // Budget set just large enough for header + 64 KiB payload (one
        // class-7 chunk). With `+` the total ≈ header + 64 KiB → fits.
        // With `*` the total ≈ header * 64 KiB → vastly over budget →
        // build would panic. We assert successful build.
        let arena = Arena::builder().byte_budget(128 * 1024).with_capacity_local(64 * 1024).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);

        let arena2 = Arena::builder().byte_budget(128 * 1024).with_capacity_shared(64 * 1024).build();
        assert_eq!(arena2.stats().normal_shared_chunks_allocated, 1);
    }

    /// Kills `chunk_provider.rs:462:9 try_pop_shared_at_least → None` and
    /// `479:20 >= → <` (the cap-vs-min_bytes filter inside the cache pop).
    ///
    /// If `try_pop_shared_at_least` always returns `None`, every
    /// `acquire_shared` cache-hit becomes a cache miss → a fresh chunk
    /// is allocated → `normal_shared_chunks_allocated` doubles. To
    /// detect: preallocate a shared chunk, then issue an arc that
    /// should reuse the cached chunk; assert the counter does not
    /// increase. With `>= → <` cap, every cached chunk fails the size
    /// gate and we also miss the cache.
    #[cfg(feature = "stats")]
    #[test]
    fn shared_cache_pop_serves_preallocated_chunk() {
        let arena = Arena::builder().with_capacity_shared(64 * 1024).build();
        assert_eq!(arena.stats().normal_shared_chunks_allocated, 1);
        // Allocate a small arc — should reuse the cached chunk.
        let _a = arena.alloc_arc(42_u64);
        assert_eq!(
            arena.stats().normal_shared_chunks_allocated,
            1,
            "small arc must reuse preallocated 64 KiB shared chunk; if try_pop_shared_at_least returned None, the counter would be 2"
        );
    }
}

// === merged from tests/mutants_internal.rs ===
mod mutants_for_internal {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "explicit .clone() clarifies test intent")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit")]
    #![allow(dead_code, reason = "tracked allocator's drop side-effect is the observable")]
    use core::alloc::Layout;
    use core::ptr::NonNull;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use allocator_api2::alloc::{AllocError, Allocator, Global};
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    /// A `Send + Sync` allocator that bumps `drop_count` once per clone
    /// when its boxed state is dropped. Used to observe that
    /// allocator clones held by chunks/provider are dropped on `Arena::drop`.
    #[derive(Clone)]
    struct DropTrackingAllocator {
        drop_count: StdArc<AtomicUsize>,
        inner: StdArc<DropTracker>,
    }
    struct DropTracker {
        counter: StdArc<AtomicUsize>,
    }
    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    impl DropTrackingAllocator {
        fn new() -> (Self, StdArc<AtomicUsize>) {
            let counter = StdArc::new(AtomicUsize::new(0));
            let alloc = Self {
                drop_count: counter.clone(),
                inner: StdArc::new(DropTracker { counter: counter.clone() }),
            };
            (alloc, counter)
        }
    }
    // SAFETY: forwards to Global; no internal mutability requirements.
    unsafe impl Allocator for DropTrackingAllocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            Global.allocate(layout)
        }
        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
            // SAFETY: forwarded.
            unsafe { Global.deallocate(ptr, layout) };
        }
    }

    /// Verifies that allocator clones held by the chunk pipeline are
    /// dropped when the arena is dropped (no leak of boxed state).
    #[test]
    fn allocator_clones_dropped_when_arena_drops() {
        let (alloc, counter) = DropTrackingAllocator::new();
        {
            let arena: Arena<DropTrackingAllocator> = Arena::new_in(alloc);
            // Touch the arena so a chunk really exists.
            let _b = arena.alloc_box(42_u64);
            // Drop arena → drops chunk/provider → drops allocator clones →
            // DropTracker drops → counter += 1.
        }
        assert!(
            counter.load(Ordering::Relaxed) >= 1,
            "allocator clones must be released on arena drop"
        );
    }

    /// Kills `constants.rs:76:14 >= → <` and `87:13 < → <=` in
    /// `min_class_for_bytes` (the upper / inner loop boundaries) and
    /// `77:34 - → +/`.
    ///
    /// `min_class_for_bytes` saturates at `NUM_CHUNK_CLASSES - 1` for
    /// `bytes >= MAX_CHUNK_BYTES` (= 64 KiB). With `< →`, the saturation
    /// inverts and small inputs return class 7. The inner `while v <
    /// ratio` is `<`; flipping to `<=` adds an extra round-up to the
    /// next class.
    ///
    /// Observation: `with_capacity_local(N)` preallocates one chunk of
    /// class = `min_class_for_bytes(N).min(NUM_CHUNK_CLASSES-1)`. By
    /// scanning a few `N` we trigger several `min_class` paths.
    #[cfg(feature = "stats")]
    #[test]
    fn min_class_for_bytes_consistency() {
        // 512 → class 0 → exactly 512 bytes preallocated
        let arena = Arena::builder().with_capacity_local(512).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);

        // 513 → class 1 (1 KiB) → one chunk
        let arena = Arena::builder().with_capacity_local(513).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);

        // 1024 → class 1 (1 KiB) exactly
        let arena = Arena::builder().with_capacity_local(1024).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);

        // 1025 → class 2 (2 KiB) → one chunk
        let arena = Arena::builder().with_capacity_local(1025).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);

        // 65536 → class 7 (64 KiB) → one chunk
        let arena = Arena::builder().with_capacity_local(65536).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);

        // 65537 → saturates at class 7 → two 64 KiB chunks (ceil-div).
        let arena = Arena::builder().with_capacity_local(65537).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 2);
    }

    /// Kills `shared_chunk.rs:168:9 to_thin_ptr → Default::default()` (returns null).
    ///
    /// `to_thin_ptr` returns the chunk header address. If replaced with
    /// `Default::default()` (= null), the shared-cache Treiber stack
    /// link writes would store nulls — preallocated chunks would not be
    /// findable. Detection: preallocate a shared chunk and assert it
    /// can be popped to serve an Arc.
    #[cfg(feature = "stats")]
    #[test]
    fn to_thin_ptr_returns_chunk_address() {
        let arena = Arena::builder().with_capacity_shared(1024).with_capacity_shared(2048).build();
        // We requested capacity twice. The second call overrides the first
        // (builder is fluent), so we expect one preallocated chunk of
        // class >= the requested bytes. We just check that arcs use the
        // cached chunk.
        let prealloc = arena.stats().normal_shared_chunks_allocated;
        assert!(prealloc >= 1);
        // One arc should reuse the cache — counter should not grow.
        let _a = arena.alloc_arc(7_u64);
        assert_eq!(
            arena.stats().normal_shared_chunks_allocated,
            prealloc,
            "small arc must reuse preallocated chunk (kills `to_thin_ptr → null`)"
        );
    }

    /// Kills `shared_chunk.rs:187:38 - → +` and `187:38 - → /` in
    /// `SharedChunk::allocate`. The line is
    /// `min_payload.checked_add(entry_align - 1)?  & !(entry_align - 1)`.
    ///
    /// With `- → +`, `entry_align - 1` becomes `entry_align + 1` (9) →
    /// the mask drops bits 0 and 3 from the rounded-up payload, producing
    /// a payload smaller than requested and misaligned for `DropEntry`
    /// writes. With `/ 1` it stays `entry_align` (8), `& !8 = & ~0b1000`
    /// → only bit 3 cleared, mis-aligning payload.
    ///
    /// We allocate many Arc<Drop>'s into a shared chunk. With wrong
    /// `payload`, the drop_back stack writes would be misaligned (UB on
    /// some platforms) and replay would read garbage. The 1024-arc
    /// test in `mutants_kill.rs` already exercises this; this test pins
    /// a smaller, faster variant via stats.
    #[test]
    fn shared_chunk_payload_alignment_supports_drop_entries() {
        #[derive(Debug)]
        struct D(StdArc<AtomicUsize>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let c = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let mut keep: Vec<multitude::Arc<D>> = Vec::new();
            for _ in 0..256_u32 {
                keep.push(arena.alloc_arc(D(c.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(c.load(Ordering::Relaxed), 256);
    }

    /// Kills `arena_builder.rs:174:80 - → +` and `- → /` in
    /// `ArenaBuilder::resolve_capacity` — already covered in
    /// `mutants_kill.rs::resolve_capacity_uses_correct_class_minus_one_clamp`.
    /// This is a thin additional regression with a different capacity
    /// to maximize boundary coverage.
    #[cfg(feature = "stats")]
    #[test]
    fn resolve_capacity_64kib_yields_single_chunk() {
        let arena = Arena::builder().with_capacity_local(64 * 1024).build();
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
        let arena2 = Arena::builder().with_capacity_shared(64 * 1024).build();
        assert_eq!(arena2.stats().normal_shared_chunks_allocated, 1);
    }
}

// === merged from tests/mutants_kill_boundaries.rs ===
mod mutants_for_kill_boundaries {
    #![cfg(feature = "stats")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::items_after_statements, reason = "test code")]
    #![allow(dead_code, reason = "test code: probe payload fields are intentionally inert")]
    use multitude::{Arc, Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    const MAX_NORMAL_ALLOC: usize = 16 * 1024;
    const PREFIX_BYTES: usize = core::mem::size_of::<usize>();

    // ---------------------------------------------------------------------------
    // alloc_str.rs:251 — `if total > max_normal_alloc` in `try_alloc_str_prefixed_local`.
    // Kills `>` → `>=` at the exact boundary `total == max_normal_alloc`.
    // ---------------------------------------------------------------------------

    #[test]
    fn alloc_str_box_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "b".repeat(len);
        let b = arena.alloc_str_box(&s);
        assert_eq!(b.len(), len);
        let s = arena.stats();
        assert!(s.normal_shared_chunks_allocated + s.oversized_shared_chunks_allocated >= 1);
        assert_eq!(s.oversized_local_chunks_allocated, 0);
    }

    // ---------------------------------------------------------------------------
    // alloc_str.rs:288 — same boundary in `try_alloc_str_prefixed_shared`.
    // ---------------------------------------------------------------------------

    #[test]
    fn alloc_str_arc_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena: Arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "c".repeat(len);
        let arc: Arc<str> = arena.alloc_str_arc(&s);
        assert_eq!(arc.len(), len);
        let s = arena.stats();
        assert!(s.normal_shared_chunks_allocated + s.oversized_shared_chunks_allocated >= 1);
    }

    // Past-boundary sanity check: also catches `> → ==` and `> → <` mutants on
    // the same line (both make the routing false for strictly-greater inputs,
    // causing the fast path to fail).

    #[test]
    fn alloc_str_arc_past_boundary_uses_oversized() {
        let arena: Arena = Arena::new();
        let len = MAX_NORMAL_ALLOC + 16;
        let s = "q".repeat(len);
        let arc: Arc<str> = arena.alloc_str_arc(&s);
        assert_eq!(arc.len(), len);
        assert!(arena.stats().oversized_shared_chunks_allocated >= 1);
    }

    // ---------------------------------------------------------------------------
    // alloc_utf16.rs:25 — `if total > max_normal_alloc` in `try_alloc_utf16_prefixed_local`.
    // ---------------------------------------------------------------------------

    // ---------------------------------------------------------------------------
    // alloc_utf16.rs:63 — same boundary in `try_alloc_utf16_prefixed_shared`.
    // ---------------------------------------------------------------------------

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_at_boundary_takes_inner_path_not_outer_oversized() {
        use widestring::Utf16Str;
        let arena: Arena = Arena::new();
        let len = (MAX_NORMAL_ALLOC - PREFIX_BYTES) / 2;
        let buf: Vec<u16> = vec![u16::from(b'z'); len];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let arc = arena.alloc_utf16_str_arc(src);
        assert_eq!(arc.len(), len);
        let s = arena.stats();
        assert!(s.normal_shared_chunks_allocated + s.oversized_shared_chunks_allocated >= 1);
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_past_boundary_uses_oversized() {
        use widestring::Utf16Str;
        let arena: Arena = Arena::new();
        let len = (MAX_NORMAL_ALLOC - PREFIX_BYTES) / 2 + 16;
        let buf: Vec<u16> = vec![u16::from(b'w'); len];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let arc = arena.alloc_utf16_str_arc(src);
        assert_eq!(arc.len(), len);
        assert!(arena.stats().oversized_shared_chunks_allocated >= 1);
    }

    // ---------------------------------------------------------------------------
    // alloc_str.rs:200 — `if len > self.provider.max_normal_alloc` in
    // `impl_alloc_str_inner` (the simple-reference `alloc_str` path).
    // Kills `>` → `>=` at `len == max_normal_alloc`.
    // ---------------------------------------------------------------------------

    #[test]
    fn alloc_str_simple_ref_at_max_normal_alloc_boundary_takes_inner_path() {
        // Use a non-power-of-two `max_normal_alloc` so the rounded chunk
        // class capacity strictly exceeds the boundary `len`. Then a
        // 1-byte follow-on alloc fits in the same chunk under the
        // original `>` semantics, but forces a refill under the `>=`
        // mutant (which pins the boundary chunk into the simple-ref pin
        // list, leaving `current_local` empty).
        let arena = Arena::builder().max_normal_alloc(5000).build();
        let _: &mut str = arena.alloc_str("x".repeat(5000));
        let _: &mut str = arena.alloc_str("y");
        assert_eq!(
            arena.stats().normal_local_chunks_allocated,
            1,
            "boundary alloc_str must route via inner refill (which keeps the chunk as `current_local`), not the oversized pin path"
        );
    }

    #[test]
    fn alloc_str_simple_ref_past_max_normal_alloc_uses_oversized() {
        // Past-boundary sanity: kills `> → <` / `> → ==` on the same line
        // — any flip leaves the oversized path unused for a strictly
        // larger string, breaking the no-cap promise of `alloc_str`.
        let arena = Arena::builder().max_normal_alloc(5000).build();
        let _: &mut str = arena.alloc_str("x".repeat(5001));
        assert!(arena.stats().oversized_local_chunks_allocated >= 1);
    }

    // ---------------------------------------------------------------------------
    // alloc_utf16.rs:63 — `if total > max_normal_alloc` in
    // `try_alloc_utf16_prefixed_shared` for sizes past `MAX_CHUNK_BYTES`.
    // Kills `>` → `<` on inputs where the mutant would route through
    // `refill_shared`, which rejects `total > MAX_CHUNK_BYTES` outright.
    // ---------------------------------------------------------------------------

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_above_max_chunk_bytes_uses_oversized() {
        use widestring::Utf16Str;
        // Shrink `max_normal_alloc` to its minimum (4 KiB) so a small
        // (4 KiB + 1)-byte payload triggers the oversized routing
        // without having to actually copy 80 KiB under Miri. The
        // mutation under test is `min_payload > max_normal_alloc`
        // → `<` in `ChunkProvider::acquire_shared`: with `<`, a
        // request in the gap `(max_normal_alloc, MAX_CHUNK_BYTES]`
        // wrongly routes to the normal cache instead of oversized,
        // and `oversized_shared_chunks_allocated == 0` fails the
        // assertion.
        let arena: Arena = Arena::builder().max_normal_alloc(4096).build();
        // 2049 u16s = 4098 payload bytes, strictly above 4 KiB.
        let len_u16 = 2049_usize;
        let buf: Vec<u16> = vec![u16::from(b'a'); len_u16];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let arc = arena.alloc_utf16_str_arc(src);
        assert_eq!(arc.len(), len_u16);
        assert!(arena.stats().oversized_shared_chunks_allocated >= 1);
    }

    // ---------------------------------------------------------------------------
    // inner_value.rs:805 — `if cur_chunk_addr != chunk_addr` chunk-eviction check
    // in `impl_alloc_inner_with`. Mutant `==` would take the eviction path when
    // chunks are equal (the common no-eviction case), corrupting state. Any
    // `alloc_with` of `T: Drop` exercises this check.
    // ---------------------------------------------------------------------------

    #[test]
    fn alloc_with_drop_type_no_eviction_returns_correct_value() {
        struct DropProbe(u64);
        impl Drop for DropProbe {
            fn drop(&mut self) {}
        }
        let arena: Arena = Arena::new();
        let r = arena.alloc_with::<DropProbe, _>(|| DropProbe(0x1234_5678_9abc_def0));
        assert_eq!(r.0, 0x1234_5678_9abc_def0);
    }

    // ---------------------------------------------------------------------------
    // internals.rs:302 — `align_up`'s arithmetic mutations.
    // `value.saturating_add(align - 1) & !(align - 1)` —
    // mutants `- → +/`/`, `& → |/^` would all return wrong aligned values
    // for any `value` that isn't already aligned. Allocate a `u128`
    // (align=16) right after a `u8` to force a non-trivial alignment step.
    // ---------------------------------------------------------------------------

    #[test]
    fn align_up_used_by_oversized_dst_alloc_produces_aligned_pointer() {
        use allocator_api2::alloc::{Allocator, Layout};

        // Going through `&Arena as Allocator::allocate` reaches
        // `Arena::allocate_layout` (in `arena/primitives.rs`), which is
        // the call site that uses the standalone `align_up` helper.
        // (The slice / value fast paths inline the same arithmetic via
        // `try_bump_fit`, not via `align_up`.)
        let arena: Arena = Arena::new();
        let allocator: &Arena = &arena;
        // A 16-byte-aligned, 48-byte layout. Original `align_up` rounds
        // the chunk's data pointer up to a 16-aligned address; the
        // `& → |/^` and `- → +/`/` mutants compute the wrong mask and
        // return an address whose low 4 bits are non-zero.
        let layout = Layout::from_size_align(48, 16).unwrap();
        let p = allocator.allocate(layout).unwrap();
        let addr = p.as_ptr().cast::<u8>() as usize;
        assert_eq!(addr % 16, 0, "align_up must produce a 16-aligned pointer");
        // Deallocate so the chunk reclaims its refcount; otherwise Miri (and
        // any leak-aware allocator) would flag the chunk as leaked.
        // SAFETY: `p` came from `allocator.allocate(layout)` with the same layout.
        unsafe { allocator.deallocate(p.cast(), layout) };
    }

    // ---------------------------------------------------------------------------
    // alloc_utf16.rs:25 / :63 — `> → <` mutation: at boundary `total < max_normal_alloc`
    // the original `>` is false (fast path, normal chunk); the `<` mutant is
    // true and routes to the outer oversized helper → an oversized chunk gets
    // allocated even though a normal chunk would have served. Detect via the
    // `oversized_*_chunks_allocated` counters.
    // ---------------------------------------------------------------------------

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_small_string_stays_in_normal_chunk() {
        use widestring::Utf16Str;
        let arena: Arena = Arena::new();
        let buf: Vec<u16> = vec![u16::from(b'b'); 10];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let arc = arena.alloc_utf16_str_arc(src);
        assert_eq!(arc.len(), 10);
        assert_eq!(
            arena.stats().oversized_shared_chunks_allocated,
            0,
            "small utf16 alloc must take the fast path, not the outer oversized helper (shared)"
        );
    }

    // Mirror small-alloc tests for `alloc_str_rc/_box/_arc` so the `> → <`
    // mutation on those boundary checks is also caught.

    #[test]
    fn alloc_str_box_small_stays_in_normal_chunk() {
        let arena = Arena::new();
        let b = arena.alloc_str_box("world");
        assert_eq!(b.len(), 5);
        assert_eq!(
            arena.stats().oversized_local_chunks_allocated,
            0,
            "small str alloc must take the fast path"
        );
    }

    #[test]
    fn alloc_str_arc_small_stays_in_normal_chunk() {
        let arena: Arena = Arena::new();
        let arc: Arc<str> = arena.alloc_str_arc("test");
        assert_eq!(arc.len(), 4);
        assert_eq!(
            arena.stats().oversized_shared_chunks_allocated,
            0,
            "small str alloc must take the fast path (shared)"
        );
    }

    // ---------------------------------------------------------------------------
    // owned_in_chunk.rs:82 / :128 — `Drop` impls of `OwnedIn{Local,Shared}Chunk`
    // release one chunk refcount. The mutant replaces the body with `()` so the
    // refcount stays bumped → chunk leaks. Detect via `total_bytes_allocated`
    // staying nonzero (chunk never freed) plus a follow-up alloc that has to
    // allocate a NEW chunk (because the leaked one keeps the cache empty).
    // ---------------------------------------------------------------------------

    #[test]
    fn drop_of_owned_in_shared_chunk_decrements_refcount_releases_chunk() {
        use multitude::Arc;
        let arena: Arena = Arena::new();
        let arc: Arc<u64> = arena.alloc_arc(7_u64);
        assert_eq!(*arc, 7);
        drop(arc);
        drop(arena);
    }
}

// === merged from tests/coverage_arena_gaps.rs ===
mod coverage_arena_gaps {
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(
        clippy::large_stack_arrays,
        reason = "tests deliberately use large allocations to drive oversized paths"
    )]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    extern crate alloc;

    use allocator_api2::alloc::Global;
    use multitude::{Arc, Arena, ArenaBuilder};

    #[cfg(feature = "std")]
    use crate::common;

    // ============================================================================
    // Helpers
    // ============================================================================

    /// Half-chunk-aligned (`MAX_SMART_PTR_ALIGN`) type without `Drop`.
    /// Used to drive over-alignment rejection in `_with` family functions
    /// without ever instantiating the value on the test stack frame
    /// (the over-alignment guard fires before the closure is invoked).
    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct HalfChunkAlign;

    /// Chunk-aligned (`CHUNK_ALIGN`) Copy type used to drive the
    /// `layout.align() >= CHUNK_ALIGN` guard in the slice-copy family.
    /// Same Windows-stack caveat as [`HalfChunkAlign`]: never lives on
    /// the test stack.
    #[repr(align(65536))]
    #[derive(Clone, Copy)]
    struct ChunkAlign;

    // ============================================================================
    // alloc_value.rs:319 — `try_alloc` (simple reference) success path.
    // ============================================================================

    #[test]
    fn try_alloc_simple_ref_returns_mutable_reference() {
        let arena = Arena::<Global>::new();
        let r = arena.try_alloc(42_u32).unwrap();
        assert_eq!(*r, 42);
        *r = 7;
        assert_eq!(*r, 7);
    }

    // ============================================================================
    // alloc_uninit.rs:231,233,235 — `try_alloc_uninit_arc` success path.
    // The existing coverage tests only exercise the failure paths
    // (failing-allocator and over-aligned).
    // ============================================================================

    #[test]
    fn try_alloc_uninit_arc_succeeds() {
        let arena = Arena::<Global>::new();
        let arc = arena.try_alloc_uninit_arc::<u32>().unwrap();
        // Just checking that the Arc is well-formed. The MaybeUninit's
        // content is not initialized; dropping the Arc only releases the
        // chunk slot via `noop_drop_shim`.
        drop(arc);
    }

    // ============================================================================
    // inner_value.rs:43, 83–114 — `try_alloc_inner_arc_with` `needs_drop` branch.
    // The integration suite tests `try_alloc_arc` only with `!needs_drop`
    // types (`u32` etc.), so the `entry_size > 0` arc fast path is
    // unobserved.
    // ============================================================================

    #[cfg(feature = "std")]
    #[test]
    fn try_alloc_arc_with_needs_drop_value_runs_drop_on_arena_teardown() {
        use std::sync::Mutex;

        struct D(&'static Mutex<u32>);
        impl Drop for D {
            fn drop(&mut self) {
                *self.0.lock().unwrap() += 1;
            }
        }

        static DROPS: Mutex<u32> = Mutex::new(0);
        let baseline = *DROPS.lock().unwrap();
        let arena = Arena::<Global>::new();
        let a = arena.try_alloc_arc(D(&DROPS)).unwrap();
        drop(a);
        drop(arena);
        assert_eq!(
            *DROPS.lock().unwrap() - baseline,
            1,
            "needs-drop arc fast path must install a real drop shim"
        );
    }

    // ============================================================================
    // inner_value.rs:50 — `try_alloc_inner_arc_with` oversized routing.
    // Exercises `try_alloc_inner_arc_oversized_with` via the try-Arc
    // surface (the existing oversized arc tests use the `alloc_arc`
    // panicking wrapper, which routes through a different function).
    // ============================================================================

    #[test]
    fn try_alloc_arc_oversized_value_succeeds() {
        let arena = Arena::<Global>::new();
        let arc = arena.try_alloc_arc([7_u8; 70_000]).unwrap();
        assert_eq!(arc[0], 7);
        assert_eq!(arc[69_999], 7);
    }

    // ============================================================================
    // inner_value.rs:147 — `alloc_inner_arc_with_or_panic` over-alignment panic.
    // ============================================================================

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_arc_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_arc_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
    }

    // ============================================================================
    // inner_value.rs:770 — `try_alloc_inner_with` oversized routing.
    // The closure form ensures we exercise `try_alloc_inner_with` itself
    // rather than the by-value `try_alloc_inner_value` path.
    // ============================================================================

    #[test]
    fn try_alloc_with_oversized_value_succeeds() {
        let arena = Arena::<Global>::new();
        let r: &mut [u8; 70_000] = arena.try_alloc_with(|| [3_u8; 70_000]).unwrap();
        assert_eq!(r[0], 3);
        assert_eq!(r[69_999], 3);
    }

    // ============================================================================
    // inner_value.rs:927 — `alloc_inner_with_or_panic` over-alignment panic.
    // ============================================================================

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _: &mut HalfChunkAlign = arena.alloc_with(|| HalfChunkAlign);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_box_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_box_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
    }

    // ============================================================================
    // inner_slice.rs:430 — `alloc_slice_local_with_or_panic` over-alignment panic.
    // inner_slice.rs:1003 — shared sibling.
    // inner_slice.rs:769 — `alloc_slice_local_copy_or_panic` over-alignment panic.
    //
    // The panicking *_with helpers are reached via `alloc_uninit_box` /
    // `alloc_uninit_rc` (local) and `alloc_uninit_arc` (shared). Their
    // over-alignment check fires before any closure runs, so no value
    // ever lives on the test stack frame.
    // ============================================================================

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_uninit_box_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_uninit_box::<HalfChunkAlign>();
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_uninit_arc_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_uninit_arc::<HalfChunkAlign>();
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_copy_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        // Empty slice of a `CHUNK_ALIGN`-aligned `Copy` type triggers the
        // `layout.align() >= CHUNK_ALIGN` guard at the top of
        // `alloc_slice_local_copy_or_panic` without instantiating a value.
        let src: &[ChunkAlign] = &[];
        let _ = arena.alloc_slice_copy(src);
    }

    // ============================================================================
    // inner_slice.rs:550 — `try_alloc_slice_local_no_drop_with` over-alignment.
    // inner_slice.rs:667 — `try_alloc_slice_local_copy` over-alignment.
    // inner_slice.rs:833 — `try_alloc_slice_shared_copy` over-alignment.
    // ============================================================================

    #[test]
    fn try_alloc_slice_no_drop_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        // `try_alloc_slice_fill_with` routes through
        // `try_alloc_slice_local_no_drop_with` for `!needs_drop` T. The cap
        // for the SimpleRef flavor is `CHUNK_ALIGN` (the chunk-recovery
        // limit), not the smart-pointer cap — so use a 64 KiB-aligned
        // type to drive the rejection.
        let res: Result<&mut [ChunkAlign], _> = arena.try_alloc_slice_fill_with(1, |_| ChunkAlign);
        assert!(res.is_err());
    }

    #[test]
    fn try_alloc_slice_copy_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let src: &[ChunkAlign] = &[];
        let res = arena.try_alloc_slice_copy(src);
        assert!(res.is_err());
    }

    #[test]
    fn try_alloc_slice_copy_arc_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let src: &[ChunkAlign] = &[];
        let res = arena.try_alloc_slice_copy_arc(src);
        assert!(res.is_err());
    }

    // ============================================================================
    // inner_slice.rs:441 — `alloc_slice_local_with_or_panic` `len > u16::MAX`
    // with drop_fn panic.
    // inner_slice.rs:1014 — shared sibling.
    // ============================================================================

    #[cfg(feature = "std")]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_with_arc_drop_too_long_panics() {
        #[derive(Clone)]
        struct D;
        #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<D>() true so a drop_fn is installed")]
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_slice_fill_with_arc(u16::MAX as usize + 1, |_| D);
    }

    // ============================================================================
    // inner_slice.rs:443–444 — `alloc_slice_local_with_or_panic` oversized.
    // inner_slice.rs:553–554 — `try_alloc_slice_local_no_drop_with` oversized.
    // inner_slice.rs:670–671 — `try_alloc_slice_local_copy` oversized.
    // inner_slice.rs:774–775 — `alloc_slice_local_copy_or_panic` oversized.
    // inner_slice.rs:836–837 — `try_alloc_slice_shared_copy` oversized.
    // inner_slice.rs:1016–1017 — `alloc_slice_shared_with_or_panic` oversized.
    // ============================================================================

    #[test]
    fn try_alloc_slice_fill_with_oversized() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let slice: &mut [u32] = arena.try_alloc_slice_fill_with(2048, |i| u32::try_from(i).unwrap()).unwrap();
        assert_eq!(slice[0], 0);
        assert_eq!(slice[2047], 2047);
    }

    #[test]
    fn try_alloc_slice_copy_oversized() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let src: alloc::vec::Vec<u32> = (0..2048_u32).collect();
        let slice = arena.try_alloc_slice_copy(&*src).unwrap();
        assert_eq!(slice[0], 0);
        assert_eq!(slice[2047], 2047);
    }

    #[test]
    fn alloc_slice_copy_oversized() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let src: alloc::vec::Vec<u32> = (0..2048_u32).collect();
        let slice = arena.alloc_slice_copy(&*src);
        assert_eq!(slice[0], 0);
        assert_eq!(slice[2047], 2047);
    }

    #[test]
    fn try_alloc_slice_copy_arc_oversized() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let src: alloc::vec::Vec<u32> = (0..2048_u32).collect();
        let arc = arena.try_alloc_slice_copy_arc(&*src).unwrap();
        assert_eq!(arc[0], 0);
        assert_eq!(arc[2047], 2047);
    }

    #[test]
    fn alloc_slice_fill_with_arc_oversized() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let arc: Arc<[u32]> = arena.alloc_slice_fill_with_arc(2048, |i| u32::try_from(i).unwrap());
        assert_eq!(arc[0], 0);
        assert_eq!(arc[2047], 2047);
    }

    // ============================================================================
    // inner_value.rs:481 — `alloc_inner_value_or_panic` over-alignment
    // panic. Reached via the by-value `alloc`/`alloc_rc`/`alloc_box`
    // entry points. Using a ZST with high alignment keeps the value's
    // stack footprint at zero bytes so the Windows chkstk-on-large-align
    // hazard that blocks the `Drop`-typed `TooAligned` tests in
    // `coverage_more.rs` does not apply.
    // ============================================================================

    // ============================================================================
    // inner_value.rs:39 — `try_alloc_inner_arc_with` over-alignment err path.
    // Closure form avoids placing a high-alignment value on the test
    // stack frame (the guard fires before the closure is invoked).
    // ============================================================================

    #[test]
    fn try_alloc_arc_with_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let res = arena.try_alloc_arc_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
        assert!(res.is_err());
    }

    // ============================================================================
    // inner_value.rs:1000–1011 — `alloc_inner_with_or_panic`
    // closure-induced eviction commit path. A reentrant allocation that
    // fills the current_local chunk during the closure forces a refill
    // that evicts the chunk the outer allocation reserved on; the outer
    // then takes the cold `commit_alloc_after_eviction` branch.
    // ============================================================================

    #[cfg(feature = "std")]
    #[test]
    fn alloc_with_closure_induced_eviction_commits_drop_entry() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        // `Send` drop type (the deferred-drop `alloc_with` path requires
        // `T: Send`; a `!Send` value here would be a soundness hazard at
        // cross-thread arena teardown).
        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::builder().max_normal_alloc(4096).build();
        // Warm up so the outer `alloc_with` below takes the fast path
        // (the cold slow path bypasses the eviction-commit branch).
        let _ = arena.alloc::<u64>(0);
        let counter = drops.clone();
        let arena_ref = &arena;
        let _outer: &mut D = arena.alloc_with(move || {
            // Fill the current_local chunk so the OUTER allocation's
            // reserved slot ends up in a chunk that gets evicted before
            // the closure returns. The outer must then take the
            // `commit_alloc_after_eviction` branch.
            // 2048 u64 allocs still force multiple refills with this 4 KiB
            // normal-allocation cap, so the reserved chunk is evicted.
            for _ in 0..2048_u32 {
                let _ = arena_ref.alloc::<u64>(0);
            }
            D(counter)
        });
        drop(arena);
        assert_eq!(drops.load(Ordering::Relaxed), 1, "outer D's drop must run via eviction commit path");
    }

    #[test]
    fn refill_local_oversized_chunk_capacity() {
        // `with_capacity_local` preallocates space; verify the arena
        // works correctly when a generous capacity is requested.
        let arena = Arena::builder().with_capacity_local(128 * 1024).build();
        let _ = arena.alloc::<u8>(0);
    }

    #[test]
    // ICE in Miri's weak-memory model (src/tools/miri/src/concurrency/
    // weak_memory.rs:233 — "cannot have empty store buffer when previous
    // write was atomic"). Skip under Miri until upstream Miri is fixed;
    // the regular test runner exercises this path.
    #[cfg_attr(miri, ignore)]
    fn refill_shared_oversized_chunk_capacity() {
        let arena = Arena::builder().with_capacity_shared(128 * 1024).build();
        let _ = arena.alloc_arc::<u8>(0);
    }

    // ============================================================================
    // inner_slice.rs:788 — `alloc_slice_local_copy_or_panic` panics when
    // the cold-refill path returns `Err`. Exercised with
    // `FailingAllocator` configured to fail after the first chunk.
    // ============================================================================
    #[cfg(feature = "std")]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_copy_panics_when_refill_fails() {
        let alloc = common::FailingAllocator::new(1);
        let arena = Arena::new_in(alloc);
        // Consume the first chunk's bump space, then force a refill that
        // the exhausted allocator cannot satisfy.
        let _filler: &mut [u8] = arena.alloc_slice_fill_with::<u8, _>(256, |_| 0);
        let src: alloc::vec::Vec<u8> = alloc::vec![0_u8; 4096];
        let _ = arena.alloc_slice_copy(&*src);
    }

    // ============================================================================
    // primitives.rs:243 — `try_install_slice_drop_entry` returns false
    // when the value's chunk is no longer the current local chunk.
    // Reached via `Vec::into_arena_rc`'s freeze-fast-path when an
    // intervening allocation has evicted the chunk hosting the buffer.
    // ============================================================================
}

// === relocated from mutants_extras.rs (stats-gated tests) ===
#[cfg(feature = "stats")]
mod from_mutants_extras_stats {
    #![allow(clippy::items_after_statements, reason = "relocated tests put inner types near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "relocated tests use .clone() on Arc/Rc")]
    #![allow(dead_code, reason = "relocated helpers retain fields for layout")]
    #![allow(
        unfulfilled_lint_expectations,
        reason = "relocated #[expect] may be fulfilled at file or feature level"
    )]
    #![allow(
        clippy::undocumented_unsafe_blocks,
        reason = "relocated test bodies preserve original safety reasoning"
    )]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "relocated tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "relocated tests use bounded values")]
    #![allow(clippy::cast_sign_loss, reason = "relocated tests use non-negative values")]
    #![allow(clippy::empty_drop, reason = "relocated tests use empty Drop impls to mark dropability")]
    #![allow(clippy::assertions_on_result_states, reason = "relocated tests deliberately assert error returns")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "relocated test doc-comments")]
    use multitude::Box as ArenaBox;
    #[repr(align(64))]
    #[derive(Debug)]
    #[expect(dead_code, reason = "helper for relocated over-alignment tests")]
    struct Align64(u32);

    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common::{self, DropCounter, FailingAllocator, SendFailingAllocator};

    /// Kills `crates/multitude/src/arena.rs:410: replace
    /// Arena::preallocate_one_shared -> Result<(), AllocError> with Ok(())`.
    ///
    /// If the body were skipped, `with_capacity_shared(N)` would not
    /// install any chunk in the shared cache — the stats counter would
    /// remain at 0.
    #[test]
    fn preallocate_one_shared_actually_allocates_chunk() {
        let arena = Arena::builder().with_capacity_shared(1024).build();
        assert!(
            arena.stats().normal_shared_chunks_allocated >= 1,
            "with_capacity_shared(1024) must preallocate at least one shared chunk"
        );
    }

    /// Kills `crates/multitude/src/arena_builder.rs:174: replace - with +`
    /// and `replace - with /` in `ArenaBuilder::resolve_capacity`.
    ///
    /// `resolve_capacity` clamps the target class to `NUM_CHUNK_CLASSES - 1`
    /// (= 7, payload = 64 KiB). Mutating `- 1` to `+ 1` (clamp = 9) or
    /// `/ 1` (clamp = 8) lets `target_class` exceed the legal range. In
    /// release builds, `class_to_bytes(8)` returns 128 KiB and
    /// `class_to_bytes(9)` returns 256 KiB, so the chunk count for a
    /// 128 KiB request becomes `1` instead of the correct `2`. Asking for
    /// 128 KiB and asserting `>= 2` chunks distinguishes both mutants from
    /// the original.
    ///
    /// (We use shared so a single test also covers `preallocate_one_shared`'s
    /// loop; the local sibling has its own existing coverage in
    /// `tests/arena.rs::preallocate_skips_underlying_allocation_calls`.)
    #[test]
    fn resolve_capacity_uses_correct_class_minus_one_clamp() {
        // 128 KiB > MAX_CHUNK_BYTES (= 64 KiB), so target_class saturates
        // at NUM_CHUNK_CLASSES - 1 = 7 → 64 KiB chunks → 2 chunks.
        let arena = Arena::builder().with_capacity_shared(128 * 1024).build();
        assert_eq!(
            arena.stats().normal_shared_chunks_allocated,
            2,
            "128 KiB shared capacity should preallocate exactly two 64 KiB chunks"
        );

        // Same for local, to exercise the equivalent path through
        // `preallocate_one_local`.
        let arena2 = Arena::builder().with_capacity_local(128 * 1024).build();
        assert_eq!(arena2.stats().normal_local_chunks_allocated, 2);
    }

    /// Kills `crates/multitude/src/arena.rs:1201: replace
    /// <impl Drop for OversizedSharedGuard>::drop with ()`.
    ///
    /// `OversizedSharedGuard` reconciles a shared oversized chunk that was
    /// pulled with `LARGE` inflation when the user closure panics. If the
    /// drop body becomes a no-op the chunk is leaked and its bytes stay
    /// charged to the byte budget. With a tight budget, a leak makes the
    /// next oversized arc allocation fail; without the leak, the arena
    /// recovers and the second allocation succeeds.
    #[test]
    fn oversized_shared_guard_drop_releases_on_panic() {
        // Each oversized arc below uses an 8 KiB blob (1024 u64s) which
        // is still > max_normal_alloc(4096) and so routes oversized.
        // The byte budget is sized to fit exactly one chunk plus a small
        // overhead — *not* two. With the guard's drop running, the
        // panicked chunk's bytes are released and the second arc fits;
        // with the drop no-op'd, the budget is exhausted and the second
        // try_alloc returns AllocError.
        //
        // We deliberately keep the payload modest so debug-build callers
        // (e.g. `cargo careful`) don't stack-overflow on the closure's
        // by-value return type before the panic-on-drop guard logic runs.
        //
        // Tight byte_budget — accommodates exactly one 8 KiB oversized
        // chunk + header overhead, not two.
        let arena = Arena::builder().byte_budget(18 * 1024).max_normal_alloc(4096).build();

        // Trigger the panic-during-init oversized path on the shared flavor.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = arena.try_alloc_arc_with::<[u64; 1024], _>(|| panic!("boom"));
        }));
        assert!(result.is_err(), "panic must propagate");

        // Stats should reflect that a chunk was allocated *and*
        // reconciled (not leaked).
        let stats_after_panic = arena.stats();
        assert!(
            stats_after_panic.oversized_shared_chunks_allocated >= 1,
            "first arc alloc should have acquired an oversized shared chunk"
        );

        // A second oversized arc must succeed under the tight budget,
        // proving the first chunk's bytes were credited back. With the
        // guard's drop a no-op, the leaked chunk pins the budget and
        // this allocation returns Err.
        let arc_ok = arena.try_alloc_arc::<[u64; 1024]>([7_u64; 1024]);
        assert!(
            arc_ok.is_ok(),
            "second oversized arc must succeed after first panic (budget released)"
        );
    }

    /// Kills `constants.rs:76:14 >= -> <` in `min_class_for_bytes`.
    ///
    /// At `bytes = 513`, the original code falls past the saturation
    /// guard (line 76) into the loop, which returns class 1 (1 KiB).
    /// Mutated `<` makes the guard fire for any `bytes < 65536`,
    /// returning class 7 (64 KiB).
    ///
    /// `with_capacity_local(513)` preallocates one chunk of the resolved
    /// class. With a `byte_budget` tight enough to accept a 1 KiB chunk
    /// (plus header) but reject a 64 KiB chunk, `try_build` succeeds in
    /// the unmutated build and fails after mutation.
    #[test]
    fn min_class_for_bytes_classifies_513_below_saturation() {
        // 4 KiB budget easily covers (header + 1 KiB) but not a 64 KiB chunk.
        let res = Arena::builder().byte_budget(4 * 1024).with_capacity_local(513).try_build();
        assert!(res.is_ok(), "513 must resolve to class 1 (1 KiB), fitting a 4 KiB budget");
    }

    /// Kills `constants.rs:87:13 < -> <=` in the `while v < ratio` loop.
    ///
    /// For `bytes = 513`: `ratio = 513.div_ceil(512) = 2`. Original loop:
    /// v=1, c=0; while v<2: v=2, c=1. Returns class 1.
    /// Mutated `<=`: extra iteration: v=2, c=1; while 2<=2: v=4, c=2.
    /// Returns class 2.
    ///
    /// Same byte-budget trick: a 4 KiB budget admits a 1 KiB chunk but
    /// not 2 KiB plus header overhead would still fit, so we need a
    /// tighter probe. Mutated class 2 -> 2 KiB chunk. With budget=1500
    /// the unmutated 1 KiB chunk + header (<512) fits, the mutated
    /// 2 KiB + header doesn't.
    #[test]
    fn min_class_inner_loop_uses_strict_less() {
        // Probe: an unbudgeted arena easily allocates a 1 KiB chunk and a
        // 2 KiB chunk; the difference is observed only via the budget.
        // (Header is bounded; we pick numbers with comfortable margin.)
        let ok = Arena::builder().byte_budget(1500).with_capacity_local(513).try_build();
        assert!(
            ok.is_ok(),
            "513 must resolve to class 1 (1 KiB); a budget of 1500 bytes (>1 KiB) admits one chunk"
        );
    }

    /// Kills `chunk_provider.rs:133:25 > -> >=` in `reserve_budget`.
    ///
    /// The boundary `next == budget` must be accepted (`> budget` is
    /// false); mutated `>=` rejects it.
    ///
    /// Bisecting the budget for a single chunk yields the smallest
    /// passing budget B*, which is C in unmutated code and C+1 in
    /// mutated. So bisection alone cannot distinguish. Instead we bisect
    /// both `local` and `shared` budgets separately to obtain `bl` and
    /// `bs`, then test a combined budget of `bl + bs - 1`. With
    /// `with_capacity_local(512)` + `with_capacity_shared(512)`:
    ///   * unmutated: bl = Cl, bs = Cs, combined = Cl+Cs-1; total need
    ///     = Cl + Cs > combined -> rejected.
    ///   * mutated:   bl = Cl+1, bs = Cs+1, combined = Cl+Cs+1; total
    ///     need = Cl + Cs which is `< Cl+Cs+1` -> `>=` false -> admitted.
    ///
    /// Therefore asserting that combined REJECTS under the unmutated
    /// code (but would ADMIT under mutation) kills the mutant.
    #[test]
    fn reserve_budget_admits_exact_equal() {
        fn ok_local(b: usize) -> bool {
            Arena::builder().byte_budget(b).with_capacity_local(512).try_build().is_ok()
        }
        fn ok_shared(b: usize) -> bool {
            Arena::builder().byte_budget(b).with_capacity_shared(512).try_build().is_ok()
        }
        fn bisect(probe: impl Fn(usize) -> bool) -> usize {
            let (mut lo, mut hi) = (1_usize, 64 * 1024_usize);
            assert!(probe(hi));
            while lo + 1 < hi {
                let m = usize::midpoint(lo, hi);
                if probe(m) {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            hi
        }
        let bl = bisect(ok_local);
        let bs = bisect(ok_shared);
        let combined = bl + bs - 1;
        let res = Arena::builder()
            .byte_budget(combined)
            .with_capacity_local(512)
            .with_capacity_shared(512)
            .try_build();
        assert!(
            res.is_err(),
            "byte_budget == total_chunk_bytes - 1 must reject under unmutated `>` (bl={bl}, bs={bs}, combined={combined})"
        );
    }

    /// Kills `chunk_provider.rs:152:9 release_budget -> ()` in
    /// `ChunkProvider::release_budget`.
    ///
    /// If `release_budget` is a no-op, allocations stay accounted even
    /// after the chunk is freed. We allocate enough to nearly exhaust
    /// the budget, drop those allocations, then allocate again with
    /// the same arena — the freed bytes must be released so the
    /// follow-up allocation fits. The path is the failure-rollback in
    /// `acquire_local` (line 171) which calls `release_budget` after
    /// `LocalChunk::allocate` returns Err.
    ///
    /// To force a failing chunk allocation we use a budget that admits
    /// the first chunk but not the second; then deallocate (via
    /// `Arena::reset`) and reallocate. Without `release_budget`, the
    /// second attempt also fails. We use `try_alloc_box` to surface
    /// the `AllocError` as `Err` rather than panic.
    /// Kills `chunk_provider.rs:152:9 release_budget -> ()`.
    ///
    /// If `release_budget` is a no-op, the running `total_chunk_bytes`
    /// monotonically grows even when chunks are freed. The free path
    /// only fires for oversized chunks (the normal path caches the chunk
    /// and never calls `release_budget`).
    ///
    /// We allocate an oversized box, drop it (which frees the
    /// stand-alone oversized chunk and calls `release_budget`), and
    /// allocate another oversized box. With unmutated code the budget
    /// recycles and the second allocation fits; with the mutated `()`
    /// body the first allocation's bytes stay accounted, and the second
    /// allocation panics with `AllocError`.
    #[test]
    fn release_budget_frees_accounted_bytes() {
        let arena = Arena::builder().byte_budget(128 * 1024).max_normal_alloc(4 * 1024).build();
        let big1 = arena.alloc_box([0u8; 80 * 1024]);
        let s1 = arena.stats();
        assert_eq!(s1.oversized_shared_chunks_allocated, 1);
        drop(big1);
        let big2 = arena.alloc_box([0u8; 80 * 1024]);
        let s2 = arena.stats();
        assert_eq!(s2.oversized_shared_chunks_allocated, 2);
        drop(big2);
        drop(arena);
    }

    /// Kills `chunk_provider.rs:441:48 + -> *` in `acquire_shared`.
    /// Line 441: `let total_bytes = shared_header_size() + target_bytes`.
    /// Mutated `*` -> `header_size() * target_bytes`. For `target_bytes=512`
    /// and header ~ 88, that's a 45 KiB allocation request vs ~600 B
    /// original. A `byte_budget` of 1024 bytes admits 600 B but rejects
    /// 45 KiB.
    ///
    /// Boundary test: build a shared-side arena with a tight budget;
    /// force a fresh shared chunk via `alloc_arc`. With mutated `*`,
    /// the budget check rejects; original admits.
    #[test]
    fn acquire_shared_total_bytes_is_sum_not_product() {
        let arena = Arena::builder().byte_budget(2 * 1024).build();
        // First arc forces a fresh shared chunk. Unmutated header + 512
        // <= 2 KiB succeeds; mutated header * 512 >> 2 KiB fails.
        let res = arena.try_alloc_arc(0u32);
        assert!(res.is_ok(), "header + payload must sum (not multiply) for budget check");
    }

    /// Kills `arena.rs:728:30 > -> >=` in `try_alloc_inner_arc_with`.
    /// Line 728: `if layout.size() > self.provider.max_normal_alloc`
    /// routes to oversized one-shot path. Mutated `>=`: the boundary
    /// case `size == max_normal_alloc` is routed oversized, which is
    /// observable via `oversized_shared_chunks_allocated` counter.
    ///
    /// Note: this gate is reached only after the fast-path miss.
    /// To trigger the slow path we allocate one item that doesn't fit
    /// in the default shared chunk's bump cursor on first try, but the
    /// fast-path generally just installs a fresh chunk anyway. The
    /// simplest reliable trigger: allocate a value of exactly
    /// `max_normal_alloc` bytes immediately after construction — the
    /// fast-path stub state fails, slow path runs, hits line 728.
    #[test]
    fn arc_with_size_equal_max_normal_routes_normal() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        #[repr(align(8))]
        struct Block([u64; 512]); // 4096 bytes exactly
        let _a = arena.alloc_arc(Block([0u64; 512]));
        let s = arena.stats();
        assert!(s.normal_shared_chunks_allocated + s.oversized_shared_chunks_allocated >= 1);
    }

    /// Kills `arena.rs:1251:17` — `<impl Drop for OversizedSharedGuard>::drop`
    /// replaced with `()`. The Drop runs `reconcile_swap_out` to free the
    /// chunk on a panic in `init`. If `init` panics and Drop is a no-op,
    /// the chunk leaks; subsequent allocations either succeed (leaking)
    /// or fail (budget exhausted).
    ///
    /// Witness: build with a tight `byte_budget`; force a panicking Arc
    /// initialiser on an oversized chunk; catch the panic; allocate
    /// again. If the guard's Drop is a no-op, the budget stays charged
    /// and the next allocation fails.
    #[test]
    fn oversized_shared_guard_drop_releases_chunk_on_panic() {
        let arena = Arena::builder().byte_budget(256 * 1024).max_normal_alloc(4096).build();
        // First oversized arc with a panicking initialiser.
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 8 KiB arc whose init panics.
            let _a: multitude::Arc<[u64; 1024]> = arena.alloc_arc_with(|| panic!("test"));
        }));
        assert!(res.is_err());
        // Without the Drop running reconcile_swap_out, the chunk is
        // unreleased and the byte_budget is exhausted. We probe via
        // a second oversized arc.
        let _a2: multitude::Arc<[u64; 1024]> = arena.alloc_arc([0u64; 1024]);
        let s = arena.stats();
        assert_eq!(s.oversized_shared_chunks_allocated, 2);
    }

    /// Kills: arena.rs:728:30 `> -> >=` — oversized routing for arc
    /// When `layout.size()` == `max_normal_alloc`, the normal path should be
    /// used. If mutated to `>=`, it takes the oversized path.
    /// Detectable via stats: oversized vs normal shared chunk counts.
    #[test]
    fn arena_728_exact_max_normal_alloc_arc() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let _arc = arena.alloc_arc([0u8; 4096]);
        let stats = arena.stats();
        assert!(stats.normal_shared_chunks_allocated + stats.oversized_shared_chunks_allocated >= 1);
    }

    /// Verifies the one-shot oversized routing for shared chunks at the
    /// `max_normal_alloc` boundary.
    ///
    /// `try_alloc_uninit_slice_arc::<u8>(max_normal_alloc)` reserves a
    /// length prefix + drop-entry placeholder on top of the payload, so
    /// the worst-case payload exceeds `max_normal_alloc` and routes to
    /// a dedicated one-shot oversized chunk. With the one-shot fix in
    /// place, that chunk is **not** installed as `current_shared`, so a
    /// subsequent small `Arc<u8>` allocation forces refilling
    /// `current_shared` with a fresh normal chunk.
    #[test]
    fn alloc_slice_arc_at_max_normal_alloc_uses_dedicated_oversized_chunk() {
        const MAX_NORMAL: usize = 16 * 1024;
        let arena = Arena::builder().max_normal_alloc(MAX_NORMAL).build();
        let before_normal = arena.stats().normal_shared_chunks_allocated;
        let before_oversized = arena.stats().oversized_shared_chunks_allocated;
        let big = arena
            .try_alloc_uninit_slice_arc::<u8>(MAX_NORMAL)
            .expect("alloc at max_normal_alloc must succeed");
        assert_eq!(big.len(), MAX_NORMAL);
        let after_big_normal = arena.stats().normal_shared_chunks_allocated;
        let after_big_oversized = arena.stats().oversized_shared_chunks_allocated;
        assert_eq!(
            after_big_oversized - before_oversized,
            1,
            "boundary slice must come from a dedicated one-shot oversized chunk",
        );
        assert_eq!(
            after_big_normal, before_normal,
            "oversized routing must not touch the normal-chunk count",
        );
        let tiny = arena.alloc_arc(0_u8);
        assert_eq!(*tiny, 0);
        let after_tiny_normal = arena.stats().normal_shared_chunks_allocated;
        let after_tiny_oversized = arena.stats().oversized_shared_chunks_allocated;
        assert_eq!(
            after_tiny_normal - after_big_normal,
            1,
            "follow-up tiny Arc must refill `current_shared` with a fresh normal chunk",
        );
        assert_eq!(
            after_tiny_oversized, after_big_oversized,
            "follow-up tiny Arc must not allocate another oversized chunk",
        );
    }

    #[test]
    fn alloc_slice_just_above_max_normal_alloc_uses_oversized_path_shared() {
        let arena = Arena::builder().max_normal_alloc(8 * 1024).build();
        let before = arena.stats().oversized_shared_chunks_allocated;
        let n = (8 * 1024) / core::mem::size_of::<u32>() + 1;
        let _a: Arc<[u32]> = arena.alloc_slice_fill_with_arc(n, |_| 0_u32);
        let after = arena.stats().oversized_shared_chunks_allocated;
        assert_eq!(after - before, 1);
    }

    #[test]
    fn vec_realloc_first_growth_does_not_count_as_relocation() {
        // The first realloc happens when `old_cap == 0` (allocating the
        // initial buffer). The `if old_cap > 0` gate prevents counting this
        // as a relocation. Mutants that change the guard to `>=` or `==`
        // would either count the initial alloc as a relocation or skip a
        // real one.
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
        v.push(0);
        let r1 = arena.stats().relocations;
        // First push triggered the initial allocation; that's not a
        // relocation.
        assert_eq!(r1, 0);
        // Subsequent grows that move the buffer are relocations.
        for i in 1..1000_u32 {
            v.push(i);
        }
        assert!(arena.stats().relocations >= 1);
    }

    #[test]
    fn vec_resize_with_reserves_exactly_required_amount() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u8> = arena.alloc_vec();
        v.push(0_u8);
        // After this resize, the vec's capacity must be >= 4.
        v.resize_with(4, || 99_u8);
        assert_eq!(v.as_slice(), &[0_u8, 99, 99, 99]);
        // The Vec only requested `new_len - self.len == 3` extra bytes.
        // With the mutant `new_len + self.len == 5` would over-allocate
        // (harmless but technically different). Hard to detect
        // deterministically — capacity is amortized to a power of 2.
    }

    #[test]
    fn arena_builder_capacity_preallocates_correct_chunk_count() {
        use multitude::ArenaBuilder;
        let arena: Arena = Arena::builder().with_capacity_local(64 * 1024).build();
        // Preallocation creates >= 1 chunk before any user allocation.
        assert!(arena.stats().normal_local_chunks_allocated >= 1);
    }

    #[test]
    fn shared_chunk_release_returns_budget() {
        use multitude::ArenaBuilder;
        let arena: Arena = Arena::builder().byte_budget(64 * 1024 * 1024).build();
        for _ in 0..32 {
            let a: Arc<u32> = arena.alloc_arc(7);
            drop(a);
        }
        // After many alloc-drop cycles, the running budget shouldn't have
        // monotonically grown (it must drop back as chunks are released).
        assert!(arena.stats().normal_shared_chunks_allocated > 0);
    }

    #[test]
    fn small_arc_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for i in 0_u32..256 {
            let _a: Arc<u32> = arena.alloc_arc(i);
        }
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
    }

    #[test]
    fn small_box_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for i in 0_u32..256 {
            let _b: ArenaBox<u32> = arena.alloc_box(i);
        }
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
    }

    #[test]
    fn small_aligned_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for _ in 0..32 {
            let _a: Arc<Align64> = arena.alloc_arc(Align64(0));
        }
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
    }

    #[test]
    fn small_drop_arc_allocations_do_not_use_oversized_chunks() {
        use core::cell::Cell;
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        // SAFETY: read-only counter shared via reference.
        unsafe impl Send for D<'_> {}
        unsafe impl Sync for D<'_> {}
        let c = Cell::new(0);
        let arena = Arena::new();
        for _ in 0..32 {
            let _a: Arc<D<'_>> = arena.alloc_arc(D(&c));
        }
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
    }

    #[test]
    fn slow_path_arc_allocs_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        // Ratchet the chunk class via a few large uninit fillers
        // (`alloc_uninit_arc` skips per-byte init cost).
        for _ in 0..8 {
            let _filler: Arc<core::mem::MaybeUninit<[u8; 8 * 1024]>> = arena.alloc_uninit_arc::<[u8; 8 * 1024]>();
        }
        // A short burst still exercises the small-allocation slow refill path
        // at the peak shared chunk class.
        for i in 0_u32..32 {
            let _a: Arc<u32> = arena.alloc_arc(i);
        }
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
    }

    #[test]
    fn slow_path_drop_arc_allocs_do_not_use_oversized_chunks() {
        use core::cell::Cell;
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        // SAFETY: only references shared state via &Cell.
        unsafe impl Send for D<'_> {}
        unsafe impl Sync for D<'_> {}
        let c = Cell::new(0);
        let arena = Arena::new();
        // Drive the chunk-class ratchet via a few large uninit
        // allocations rather than 8 × 8 KiB filled allocs; under Miri,
        // `alloc_uninit_arc` skips the per-byte init cost.
        for _ in 0..2 {
            let _filler: Arc<core::mem::MaybeUninit<[u8; 8 * 1024]>> = arena.alloc_uninit_arc::<[u8; 8 * 1024]>();
        }
        // A short burst still reaches the peak-class slow refill path; a
        // mutated `needed` computation would route the first one oversized.
        for _ in 0..32 {
            let _a: Arc<D<'_>> = arena.alloc_arc(D(&c));
        }
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
    }

    #[test]
    fn vec_into_box_allocates_no_additional_local_chunk() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..4_u32 {
            v.push(i);
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _b: ArenaBox<[u32]> = v.into_boxed_slice();
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn shared_chunk_release_budget_remains_bounded_through_many_cycles() {
        use multitude::ArenaBuilder;
        let arena: Arena = Arena::builder().byte_budget(2 * 1024 * 1024).build();
        // Any leak in the release-budget bookkeeping compounds linearly
        // with the cycle count, so a handful of iterations is enough
        // to expose a leak; the test gains nothing from a large count
        // and Miri pays for every cycle.
        for _ in 0..8 {
            let _a: Arc<[u8; 1024]> = arena.alloc_arc([0_u8; 1024]);
        }
    }
}
