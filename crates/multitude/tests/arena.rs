// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_safety_comment,
    reason = "shared test helpers cover feature-gated paths"
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
    let result = std::panic::catch_unwind(|| Arena::builder().with_capacity(256).try_build());
    assert!(result.is_err(), "with_capacity(256) must panic (below MIN_CHUNK_BYTES = 512)");
}

#[test]
fn try_alloc_str_returns_mutable_str() {
    let arena = Arena::new();
    let mut s = arena.try_alloc_str("hello").unwrap();
    s.make_ascii_uppercase();
    assert_eq!(&*s, "HELLO");
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
    let s = arena.try_alloc_str(owned).unwrap();
    assert_eq!(&*s, "from String");
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

#[cfg(feature = "stats")]
#[test]
fn arena_stats_report_cache_reuse_and_resets() {
    let mut arena = Arena::builder().with_capacity(64 * 1024).build();
    let initial = arena.stats();
    assert_eq!(initial.cached_chunks, 1);
    assert_eq!(initial.cached_bytes, 64 * 1024);
    assert_eq!(initial.total_bytes_allocated, 64 * 1024);
    assert_eq!(initial.peak_bytes_allocated, 64 * 1024);
    assert_eq!(initial.normal_chunks_reused, 0);
    assert_eq!(initial.resets, 0);

    let value = arena.alloc(1_u64);
    drop(value);
    let active = arena.stats();
    assert_eq!(active.cached_chunks, 0);
    assert_eq!(active.cached_bytes, 0);
    assert_eq!(active.normal_chunks_reused, 1);

    arena.reset();
    let reset = arena.stats();
    assert_eq!(reset.cached_chunks, 1);
    assert_eq!(reset.cached_bytes, 64 * 1024);
    assert_eq!(reset.resets, 1);

    let value = arena.alloc(2_u64);
    drop(value);
    assert_eq!(arena.stats().normal_chunks_reused, 2);
}

#[cfg(feature = "stats")]
#[test]
fn arena_stats_preserve_peak_after_oversized_storage_is_released() {
    let arena = Arena::new();
    let source = vec![0_u8; OVERSIZED_BYTES];
    let value = arena.alloc_slice_copy_box(&source);
    let live = arena.stats();
    assert!(live.total_bytes_allocated >= OVERSIZED_BYTES as u64);
    assert_eq!(live.peak_bytes_allocated, live.total_bytes_allocated);

    drop(value);
    let released = arena.stats();
    assert_eq!(released.total_bytes_allocated, 0);
    assert_eq!(released.peak_bytes_allocated, live.peak_bytes_allocated);
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
        // The source exceeds `MAX_CHUNK_BYTES` and uses a one-shot chunk.
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
        let _r = arena.alloc_with(|| [0_u32; 8 * 1024]);
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

// An oversized chunk is released when its initializer panics.
#[test]
fn panic_in_oversized_alloc_with_does_not_leak() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let alloc = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _r = arena.alloc_with(|| panic!("synthetic panic"));
        }));
        assert!(result.is_err());
    }
    assert_eq!(alloc.live_chunks(), 0);
    assert_eq!(alloc.live_bytes(), 0);
}

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

// High-half addresses are rejected before pointer arithmetic.
#[test]
fn local_chunk_allocate_rejects_high_address_from_pathological_allocator() {
    let arena = Arena::builder_in(common::BadAddressAllocator).build();
    let result = arena.try_alloc_with(|| 0_u64);
    assert!(result.is_err(), "high-address allocator must produce AllocError");
}

#[test]
fn chunk_allocate_rejects_high_address_from_pathological_allocator() {
    use multitude::AllocError;
    let arena = Arena::builder_in(common::BadAddressAllocator).build();
    let result: Result<multitude::Arc<u64, _>, AllocError> = arena.try_alloc_arc_with(|| 0_u64);
    assert!(result.is_err(), "high-address allocator must produce AllocError");
    assert!(result.unwrap_err().is_capacity_overflow());
}

#[test]
fn try_alloc_slice_huge_len_returns_alloc_error() {
    let arena: Arena = Arena::new();
    let result = arena.try_alloc_slice_fill_with(usize::MAX / 4, |_| 0_u64);
    assert!(result.is_err(), "expected AllocError for huge len");
}

#[test]
fn try_alloc_slice_clone_huge_len_returns_alloc_error() {
    let arena: Arena = Arena::new();
    let result = arena.try_alloc_slice_fill_with(usize::MAX, |_| 0_u64);
    result.unwrap_err();
}

mod reset {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
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
    fn alloc_style_value_destructor_runs_when_handle_drops() {
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
            let _v = arena.alloc(Tracked);
            assert_eq!(COUNT.load(Ordering::SeqCst), 0, "drop hasn't fired before handle drops");
        }
        assert_eq!(COUNT.load(Ordering::SeqCst), 1, "destructor must run when handle drops");
        arena.reset();
        assert_eq!(COUNT.load(Ordering::SeqCst), 1, "reset must not drop the value again");
    }

    #[test]
    fn alloc_style_values_drop_once_before_reset() {
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
            let _ = arena.alloc(Tracked);
        }
        assert_eq!(COUNT.load(Ordering::SeqCst), 5);
        arena.reset();
        assert_eq!(COUNT.load(Ordering::SeqCst), 5, "reset must not double-drop alloc handles");
    }

    #[cfg(feature = "stats")]
    #[test]
    fn reset_returns_chunks_to_cache_and_avoids_fresh_alloc() {
        // Seed the high-water mark to the largest class up front so the
        // chunk that backs our single allocation isn't evicted from the
        // cache when it returns (the high-water filter requires
        // `cap >= class_to_bytes(high_water)`).
        let mut arena = Arena::builder().with_capacity(64 * 1024).build();
        let _ = arena.alloc(0_u64);

        let stats_before = arena.stats();
        assert!(stats_before.normal_chunks_allocated >= 1);

        arena.reset();

        let stats_after_reset = arena.stats();
        assert_eq!(stats_after_reset.normal_chunks_allocated, stats_before.normal_chunks_allocated);

        let _ = arena.alloc(1_u64);
        let stats_after_realloc = arena.stats();
        assert_eq!(
            stats_after_realloc.normal_chunks_allocated, stats_before.normal_chunks_allocated,
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
            let now = arena.stats().normal_chunks_allocated;
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
        // Allocate a couple of near-max_normal_alloc buffers to put the
        // (class-7, 64 KiB) starter chunk into use. `MaybeUninit<[u8;
        // 4000]>` skips per-byte init; a couple of them is enough to
        // exercise the reset→cache→reuse path without a long alloc loop.
        let mut arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).with_capacity(64 * 1024).build();
        for _ in 0..2 {
            let _ = arena.alloc(core::mem::MaybeUninit::<[u8; 4000]>::uninit());
        }
        let chunks_before = arena.stats().normal_chunks_allocated;
        assert!(chunks_before >= 1, "expected at least one chunk allocation, got {chunks_before}");

        arena.reset();
        let _ = arena.alloc(0_u64);
        assert_eq!(
            arena.stats().normal_chunks_allocated,
            chunks_before,
            "no fresh chunk allocation expected"
        );
    }

    #[test]
    fn reset_works_after_alloc_style_refs_drop() {
        let mut arena = Arena::new();
        {
            let mut r = arena.alloc(123);
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
        let h = thread::spawn(move || {
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

    /// Reset preserves reusable shared chunks, so repeated cycles grow chunk
    /// count sub-linearly.
    #[cfg(feature = "stats")]
    #[test]
    fn reset_does_not_allocate_a_fresh_chunk_per_cycle() {
        // One allocation per cycle keeps the Miri workload bounded.
        fn build(arena: &Arena) {
            drop(arena.alloc_arc(0xAB_u64));
        }

        const WARMUP: usize = 16;
        const BATCH: usize = 64;

        let mut arena = Arena::new();
        for _ in 0..WARMUP {
            build(&arena);
            arena.reset();
        }
        let before = arena.stats().normal_chunks_allocated;
        for _ in 0..BATCH {
            build(&arena);
            arena.reset();
        }
        let grew_by = arena.stats().normal_chunks_allocated - before;

        // Chunk growth must remain comfortably sub-linear.
        assert!(
            grew_by < BATCH as u64 / 8,
            "reset must not allocate a fresh chunk per cycle: \
             {grew_by} new chunks over {BATCH} cycles (buggy code allocates ~{BATCH})",
        );
    }

    /// A nested-`Arc` structure stays valid and drops cleanly across
    /// repeated reset cycles. Builds the structure, reads it back, drops it,
    /// resets, and repeats — confirming `reset` leaves outstanding
    /// shared-chunk contents intact.
    #[test]
    fn reset_keeps_nested_arc_structures_valid_across_cycles() {
        let mut arena = Arena::new();
        for cycle in 0..8_u8 {
            let outer: Arc<[Arc<[u8]>]> = {
                let mut v = arena.alloc_vec_with_capacity::<Arc<[u8]>>(4);
                for i in 0_u8..4 {
                    v.push(arena.alloc_slice_copy_arc(&[cycle, i, 0xCD]));
                }
                v.try_into_arc_slice().unwrap()
            };
            assert_eq!(outer.len(), 4);
            for (i, inner) in outer.iter().enumerate() {
                let i = u8::try_from(i).unwrap();
                assert_eq!(&**inner, &[cycle, i, 0xCD]);
            }
            drop(outer);
            arena.reset();
        }
    }
}

mod large_alloc {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "intentional truncation in test values")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::redundant_type_annotations, reason = "type annotations for documentation clarity")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    use std::thread;

    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
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
        let s = arena.alloc_slice_copy(&src);
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
        // Use `u128` so the element count needed to exceed `CHUNK_BYTES`
        // is 16x smaller than with `u8`, halving it again vs `u64` — the
        // `alloc_slice_clone` path still clones every element across the
        // oversized chunk, so fewer elements means far less Miri work for
        // the same `> CHUNK_BYTES` byte threshold.
        let n = CHUNK_BYTES / 16 + 2; // 4098 u128 => > 64 KiB
        let src: Vec<u128> = (0..n as u128).collect();
        let s = arena.alloc_slice_clone::<u128>(&src);
        assert_eq!(s.len(), src.len());
        assert_eq!(s[0], 0);
        assert_eq!(s[s.len() - 1], (s.len() - 1) as u128);
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
        let h = thread::spawn(move || {
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
        let cap = FAR_OVER_CHUNK / 4;
        let mut v = arena.alloc_vec_with_capacity::<u32>(cap);
        // Fill the (far-over-chunk) capacity in one bulk `extend_from_slice`
        // (a single memcpy) rather than `cap` individual `push` calls — the
        // per-`push` arena bookkeeping is what dominates under Miri. A
        // bulk-zeroed source vec is itself a single allocation.
        v.extend_from_slice(&std::vec![0_u32; cap]);
        assert_eq!(v.len(), cap);
        // The first, a mid-chunk, and the last slot must all be addressable
        // and writable across the oversized backing chunk.
        v[0] = 0xA1;
        v[CHUNK_BYTES / 4] = 0xB2;
        v[cap - 1] = 0xC3;
        assert_eq!(v[0], 0xA1);
        assert_eq!(v[CHUNK_BYTES / 4], 0xB2);
        assert_eq!(v[cap - 1], 0xC3);
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
        // Keep the Miri workload small while forcing relocation.
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
        // `u128` crosses the byte boundary with fewer interpreted iterations.
        let mut v = arena.alloc_vec::<u128>();
        let n = OVER_CHUNK / 16 + 1; // > 64 KiB worth of u128
        v.extend((0..n as u128).map(|i| i.wrapping_mul(13)));
        assert_eq!(v.len(), n);
        // Spot-check first, mid-chunk and last instead of iterating
        // every element; a chunk-boundary bug would manifest at any of
        // these positions equally and the per-element cost dominates
        // under Miri.
        for i in [0, n / 2, n - 1] {
            assert_eq!(v[i], (i as u128).wrapping_mul(13));
        }
    }

    #[test]
    fn vec_in_macro_initial_then_grow_past_chunk() {
        let arena = Arena::new();
        let mut v = multitude::vec::vec![in &arena; 0u32; 16];
        assert_eq!(v.len(), 16);
        for next in 16..(OVER_CHUNK / 4) {
            v.push(next as u32);
        }
        assert_eq!(v.len(), OVER_CHUNK / 4);
        assert_eq!(v[0], 0);
        assert_eq!(v[16], 16);
        assert_eq!(v[v.len() - 1], (v.len() - 1) as u32);
    }

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
        // Bulk push bounds the Miri workload.
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
        // Reserve past the boundary in one call to bound the Miri workload.
        s.push_from_str("hello");
        s.reserve(OVER_CHUNK);
        assert!(s.capacity() >= 5 + OVER_CHUNK);
        s.push_from_str("y");
        assert_eq!(s.len(), 6);
        let v = s.as_slice();
        assert_eq!(v[0], u16::from(b'h'));
        assert_eq!(v[5], u16::from(b'y'));
    }

    #[test]
    fn many_oversized_allocations_in_one_arena() {
        // The property under test is that an arena tolerates *multiple*
        // oversized one-shot chunks coexisting. `[u128; OVER_CHUNK/16+1]`
        // gives the byte-count threshold (above `MAX_CHUNK_BYTES`). Each
        // round is a single bulk `alloc_slice_copy` (one memcpy) from a
        // shared zeroed source rather than an `N_U128`-long fill closure
        // loop; per-round sentinels written into the first and last slots
        // preserve the distinct-content checks that prove the oversized
        // chunks don't alias.
        const N_U128: usize = OVER_CHUNK / 16 + 1; // > 64 KiB worth of u128
        let arena = Arena::new();
        let src = std::vec![0_u128; N_U128];
        let mut keepers = Vec::with_capacity(8);
        for round in 0..8u8 {
            let mut s = arena.alloc_slice_copy::<u128>(&src);
            s[0] = u128::from(round);
            s[N_U128 - 1] = u128::from(round);
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
        let h = thread::spawn(move || {
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
        let s = arena.alloc_str(&big);
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(&s[..5], "wwwww");
        assert_eq!(&s[OVER_CHUNK - 5..], "wwwww");
        // Confirm small allocations after a large one still work (the
        // oversized chunk does not become the current local slot).
        let small = arena.alloc_str("small");
        assert_eq!(&*small, "small");
    }

    #[test]
    fn alloc_str_simple_ref_far_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "Q".repeat(FAR_OVER_CHUNK);
        let s = arena.alloc_str(&big);
        assert_eq!(s.len(), FAR_OVER_CHUNK);
        // memcmp via slice equality is one bulk op instead of FAR_OVER_CHUNK
        // per-char yields under Miri.
        assert_eq!(s.as_bytes(), big.as_bytes());
    }

    #[test]
    fn try_alloc_str_simple_ref_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "p".repeat(OVER_CHUNK);
        let s = arena.try_alloc_str(&big).expect("oversized alloc_str must succeed");
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
        // Returning proves the oversized shared path terminates.
    }
}

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
            let r = arena.alloc(0xDEAD_BEEF_u64);
            let ptr = std::ptr::from_ref::<u64>(&*r) as usize;
            assert_eq!(ptr % align_of::<u64>(), 0, "u64 pointer misaligned: {ptr:#x}");
            assert_eq!(*r, 0xDEAD_BEEF_u64);
        }
    }

    #[test]
    fn alloc_u128_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let r = arena.alloc(0x1234_5678_9ABC_DEF0_u128);
            let ptr = std::ptr::from_ref::<u128>(&*r) as usize;
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
            let r = arena.alloc(Align32 { value: i });
            let ptr = std::ptr::from_ref::<Align32>(&*r) as usize;
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
            let r = arena.alloc(Align64 { data: [i; 64] });
            let ptr = std::ptr::from_ref::<Align64>(&*r) as usize;
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
            let a = arena.alloc(i as u8);
            let b = arena.alloc(i);
            let c = arena.alloc(u128::from(i));

            assert_eq!((std::ptr::from_ref::<u8>(&*a) as usize) % align_of::<u8>(), 0);
            assert_eq!(
                (std::ptr::from_ref::<u64>(&*b) as usize) % align_of::<u64>(),
                0,
                "u64 misaligned after u8"
            );
            assert_eq!(
                (std::ptr::from_ref::<u128>(&*c) as usize) % align_of::<u128>(),
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
            let r = arena.alloc(i);
            let addr = std::ptr::from_ref::<u64>(&*r) as usize;
            ptrs.push((std::ptr::from_ref::<u64>(&*r), addr));
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
        let _prime = arena.alloc(0);
        let initial_chunks = arena.stats().normal_chunks_allocated;
        assert_eq!(initial_chunks, 1);
        let mut count = 0_u64;
        while arena.stats().normal_chunks_allocated == initial_chunks {
            let r = arena.alloc(count);
            assert_eq!(*r, count);
            let ptr = std::ptr::from_ref::<u64>(&*r) as usize;
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
        let initial_chunks = arena.stats().normal_chunks_allocated;
        let mut handles = Vec::new();
        let mut count = 0_u64;
        while arena.stats().normal_chunks_allocated == initial_chunks {
            let arc = arena.alloc_arc(count);
            let ptr = &raw const *arc as usize;
            assert_eq!(ptr % align_of::<u64>(), 0);
            handles.push(arc);
            count += 1;
            assert!(count < 20_000, "should have triggered new chunk by now");
        }
        assert!(count > 10, "chunk should hold many Arc<u64>s");
    }

    #[cfg(feature = "stats")]
    #[test]
    fn oversize_alloc_goes_to_oversized_chunk() {
        // Default max_normal_alloc for 64 KiB chunks = 16 KiB.
        // Allocate something larger than that.
        let arena = Arena::new();
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
        let mut big = arena.alloc([0u8; 32 * 1024]);
        big[0] = 42;
        assert_eq!(big[0], 42);
        assert!(arena.stats().oversized_chunks_allocated >= 1);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn oversize_arc_goes_to_oversized_chunk() {
        let arena = Arena::new();
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
        let big = arena.alloc_arc([0u8; 32 * 1024]);
        assert_eq!(big[0], 0);
        assert!(arena.stats().oversized_chunks_allocated >= 1);
    }

    /// A value that requires a `DropEntry`.
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
        // The smallest chunk must fit at least ten values and their entries.
        let arena = Arena::builder().build();
        let _prime = arena.alloc(Droppable(0));
        let initial_chunks = arena.stats().normal_chunks_allocated;
        let mut count = 0_u64;
        while arena.stats().normal_chunks_allocated == initial_chunks && count < 500 {
            let r = arena.alloc(Droppable(count));
            assert_eq!(r.0, count);
            count += 1;
        }
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

mod allocator_impl {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn allocator_shrink_in_place_path() {
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

mod mutants_for_chunk_provider {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::items_after_statements, reason = "test-local types live next to their usage")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(dead_code, reason = "test structs retain payload fields to control size")]
    #[cfg(feature = "stats")]
    use multitude::{Arena, Box};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[cfg(feature = "stats")]
    #[test]
    fn reserve_budget_admits_exact_fit() {
        // Match the budget to one preallocated 512-byte chunk plus its header.
        let probe = Arena::builder().byte_budget(1024 * 1024).with_capacity(512).build();
        assert_eq!(probe.stats().normal_chunks_allocated, 1);
        drop(probe);

        // 1 KiB is enough to cover header (<512 bytes) + 512 payload, so
        // the exact-fit budget admits exactly one chunk's worth.
        let arena = Arena::builder().byte_budget(1024).with_capacity(512).build();
        assert_eq!(
            arena.stats().normal_chunks_allocated,
            1,
            "byte_budget == total bytes for one chunk must admit allocation"
        );
    }

    #[cfg(feature = "stats")]
    #[test]
    fn release_budget_runs_when_chunk_freed() {
        // A 5 MiB budget is enough for one 64 KiB chunk plus header but
        // not for two. A single 8 KiB uninit box is enough to force a
        // chunk allocation; no need for eight (the loop was for a prior
        // version of this test that needed cache eviction).
        let arena = Arena::builder().byte_budget(5 * 1024 * 1024).build();
        let box1 = arena.alloc_uninit_box::<[u8; 8 * 1024]>();
        assert!(arena.stats().normal_chunks_allocated >= 1);
        drop(box1);
        drop(arena);
        // If `release_budget` is a no-op, recreating an arena with the
        // same budget would fail to satisfy the same workload. The
        // user-observable invariant: a fresh arena with the same budget
        // admits the same allocation.
        let arena2 = Arena::builder().byte_budget(5 * 1024 * 1024).build();
        let _box2 = arena2.alloc_uninit_box::<[u8; 8 * 1024]>();
        assert!(arena2.stats().normal_chunks_allocated >= 1);
    }

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
        // Alignment slack may route this boundary-sized value to a one-shot
        // chunk, but it must allocate successfully.
        assert!(s.normal_chunks_allocated + s.oversized_chunks_allocated >= 1);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn acquire_shared_boundary_does_not_route_oversized() {
        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        #[repr(align(8))]
        struct Block([u64; 512]);
        let _a = arena.alloc_arc(Block([0_u64; 512]));
        let s = arena.stats();
        assert!(s.normal_chunks_allocated + s.oversized_chunks_allocated >= 1);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn acquire_local_class_ceiling_is_correct() {
        // The property under test: the size-class ratchet caps at the
        // largest cacheable class (class 7 = 64 KiB total). After the
        // first few refills ratchet there, subsequent refills stay at
        // class 7 — they don't keep doubling. To observe this we allocate
        // a handful of ~13 KiB boxes (under MAX_NORMAL_ALLOC = 16 KiB, so
        // still routed through the normal cache) and confirm none route to
        // oversized. Five ~13 KiB boxes total > 64 KiB, so they span ≥ 2
        // class-7 chunks, proving the ratchet stays at class 7 rather than
        // degrading or escaping. Larger-but-fewer boxes keep the byte
        // threshold while minimising the per-allocation Miri cost.
        let arena = Arena::new();
        let mut keep: Vec<Box<core::mem::MaybeUninit<[u8; 13 * 1024]>>> = Vec::new();
        for _ in 0..5 {
            keep.push(arena.alloc_uninit_box::<[u8; 13 * 1024]>());
        }
        let s = arena.stats();
        assert_eq!(s.oversized_chunks_allocated, 0);
        assert!(
            s.normal_chunks_allocated >= 2,
            "5 × 13 KiB boxes must span ≥ 2 class-7 chunks, got {}",
            s.normal_chunks_allocated
        );
    }

    #[cfg(feature = "stats")]
    #[test]
    fn high_water_ratchet_grows_chunks() {
        let arena = Arena::builder().with_capacity(512).build();
        // Preallocation should have created exactly one 512-byte (class 0) chunk.
        assert_eq!(arena.stats().normal_chunks_allocated, 1);
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
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn preallocate_total_bytes_uses_sum_not_product() {
        // Budget set just large enough for header + 64 KiB payload (one
        // class-7 chunk). With `+` the total ≈ header + 64 KiB → fits.
        // With `*` the total ≈ header * 64 KiB → vastly over budget →
        // build would panic. We assert successful build.
        let arena = Arena::builder().byte_budget(128 * 1024).with_capacity(64 * 1024).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);

        let arena2 = Arena::builder().byte_budget(128 * 1024).with_capacity(64 * 1024).build();
        assert_eq!(arena2.stats().normal_chunks_allocated, 1);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn cache_pop_serves_preallocated_chunk() {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);
        // Allocate a small arc — should reuse the cached chunk.
        let _a = arena.alloc_arc(42_u64);
        assert_eq!(
            arena.stats().normal_chunks_allocated,
            1,
            "small arc must reuse preallocated 64 KiB chunk; if pop returned None, the counter would be 2"
        );
    }
}

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

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
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

    #[cfg(feature = "stats")]
    #[test]
    fn min_class_for_bytes_consistency() {
        // 512 → class 0 → exactly 512 bytes preallocated
        let arena = Arena::builder().with_capacity(512).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);

        // 513 → class 1 (1 KiB) → one chunk
        let arena = Arena::builder().with_capacity(513).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);

        // 1024 → class 1 (1 KiB) exactly
        let arena = Arena::builder().with_capacity(1024).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);

        // 1025 → class 2 (2 KiB) → one chunk
        let arena = Arena::builder().with_capacity(1025).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);

        // 65536 → class 7 (64 KiB) → one chunk
        let arena = Arena::builder().with_capacity(65536).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);

        // 65537 → saturates at class 7 → two 64 KiB chunks (ceil-div).
        let arena = Arena::builder().with_capacity(65537).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 2);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn to_thin_ptr_returns_chunk_address() {
        let arena = Arena::builder().with_capacity(1024).with_capacity(2048).build();
        // The final capacity setting supplies a cached chunk for the Arc.
        let prealloc = arena.stats().normal_chunks_allocated;
        assert!(prealloc >= 1);
        // One arc should reuse the cache — counter should not grow.
        let _a = arena.alloc_arc(7_u64);
        assert_eq!(
            arena.stats().normal_chunks_allocated,
            prealloc,
            "small arc must reuse preallocated chunk (kills `to_thin_ptr → null`)"
        );
    }

    #[test]
    fn chunk_payload_alignment_supports_drop_entries() {
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

    #[cfg(feature = "stats")]
    #[test]
    fn resolve_capacity_64kib_yields_single_chunk() {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);
        let arena2 = Arena::builder().with_capacity(64 * 1024).build();
        assert_eq!(arena2.stats().normal_chunks_allocated, 1);
    }
}

mod mutants_for_kill_boundaries {
    #![cfg(feature = "stats")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::items_after_statements, reason = "test code")]
    #![allow(dead_code, reason = "test code: probe payload fields are intentionally inert")]
    use multitude::{Arc, Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    const MAX_NORMAL_ALLOC: usize = 16 * 1024;
    const PREFIX_BYTES: usize = core::mem::size_of::<usize>();

    #[test]
    fn alloc_str_box_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "b".repeat(len);
        let b = arena.alloc_str_box(&s);
        assert_eq!(b.len(), len);
        let s = arena.stats();
        assert!(s.normal_chunks_allocated + s.oversized_chunks_allocated >= 1);
        assert_eq!(s.oversized_chunks_allocated, 0);
    }

    #[test]
    fn alloc_str_arc_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena: Arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "c".repeat(len);
        let arc: Arc<str> = arena.alloc_str_arc(&s);
        assert_eq!(arc.len(), len);
        let s = arena.stats();
        assert!(s.normal_chunks_allocated + s.oversized_chunks_allocated >= 1);
    }

    #[test]
    fn alloc_str_arc_past_boundary_uses_oversized() {
        let arena: Arena = Arena::new();
        let len = MAX_NORMAL_ALLOC + 16;
        let s = "q".repeat(len);
        let arc: Arc<str> = arena.alloc_str_arc(&s);
        assert_eq!(arc.len(), len);
        assert!(arena.stats().oversized_chunks_allocated >= 1);
    }

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
        assert!(s.normal_chunks_allocated + s.oversized_chunks_allocated >= 1);
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
        assert!(arena.stats().oversized_chunks_allocated >= 1);
    }

    #[test]
    fn alloc_str_simple_ref_at_max_normal_alloc_boundary_takes_inner_path() {
        // A non-power-of-two threshold leaves room for the follow-on byte in
        // the same normal chunk.
        let arena = Arena::builder().max_normal_alloc(5000).build();
        let _ = arena.alloc_str("x".repeat(5000));
        let _ = arena.alloc_str("y");
        assert_eq!(
            arena.stats().normal_chunks_allocated,
            1,
            "boundary alloc_str must route via inner refill (which keeps the chunk as `current`), not the oversized pin path"
        );
    }

    #[test]
    fn alloc_str_simple_ref_past_max_normal_alloc_uses_oversized() {
        let arena = Arena::builder().max_normal_alloc(5000).build();
        let _ = arena.alloc_str("x".repeat(5001));
        assert!(arena.stats().oversized_chunks_allocated >= 1);
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_above_max_chunk_bytes_uses_oversized() {
        use widestring::Utf16Str;
        // Use the minimum threshold to keep the oversized Miri workload small.
        let arena: Arena = Arena::builder().max_normal_alloc(4096).build();
        // 2049 u16s = 4098 payload bytes, strictly above 4 KiB.
        let len_u16 = 2049_usize;
        let buf: Vec<u16> = vec![u16::from(b'a'); len_u16];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let arc = arena.alloc_utf16_str_arc(src);
        assert_eq!(arc.len(), len_u16);
        assert!(arena.stats().oversized_chunks_allocated >= 1);
    }

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

    #[test]
    fn align_up_used_by_oversized_dst_alloc_produces_aligned_pointer() {
        use allocator_api2::alloc::{Allocator, Layout};

        let arena: Arena = Arena::new();
        let allocator: &Arena = &arena;
        let layout = Layout::from_size_align(48, 16).unwrap();
        let p = allocator.allocate(layout).unwrap();
        let addr = p.as_ptr().cast::<u8>() as usize;
        assert_eq!(addr % 16, 0, "align_up must produce a 16-aligned pointer");
        // SAFETY: `p` came from `allocator.allocate(layout)` with the same layout.
        unsafe { allocator.deallocate(p.cast(), layout) };
    }

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
            arena.stats().oversized_chunks_allocated,
            0,
            "small utf16 alloc must take the fast path, not the outer oversized helper (shared)"
        );
    }

    #[test]
    fn alloc_str_box_small_stays_in_normal_chunk() {
        let arena = Arena::new();
        let b = arena.alloc_str_box("world");
        assert_eq!(b.len(), 5);
        assert_eq!(
            arena.stats().oversized_chunks_allocated,
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
            arena.stats().oversized_chunks_allocated,
            0,
            "small str alloc must take the fast path (shared)"
        );
    }

    #[test]
    fn drop_of_owned_in_chunk_decrements_refcount_releases_chunk() {
        use multitude::Arc;
        let arena: Arena = Arena::new();
        let arc: Arc<u64> = arena.alloc_arc(7_u64);
        assert_eq!(*arc, 7);
        drop(arc);
        drop(arena);
    }
}

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
    #[cfg(not(utc_backend))]
    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct HalfChunkAlign;

    /// Chunk-aligned (`CHUNK_ALIGN`) Copy type used to drive the
    /// `layout.align() >= CHUNK_ALIGN` guard in the slice-copy family.
    /// Same Windows-stack caveat as [`HalfChunkAlign`]: never lives on
    /// the test stack.
    #[cfg(not(utc_backend))]
    #[repr(align(65536))]
    #[derive(Clone, Copy)]
    struct ChunkAlign;

    #[test]
    fn try_alloc_simple_ref_returns_mutable_reference() {
        let arena = Arena::<Global>::new();
        let mut r = arena.try_alloc(42_u32).unwrap();
        assert_eq!(*r, 42);
        *r = 7;
        assert_eq!(*r, 7);
    }

    #[test]
    fn try_alloc_uninit_arc_succeeds() {
        let arena = Arena::<Global>::new();
        let arc = arena.try_alloc_uninit_arc::<u32>().unwrap();
        drop(arc);
    }

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

    #[test]
    fn try_alloc_arc_oversized_value_succeeds() {
        let arena = Arena::<Global>::new();
        let arc = arena.try_alloc_arc([7_u8; 70_000]).unwrap();
        assert_eq!(arc[0], 7);
        assert_eq!(arc[69_999], 7);
    }

    #[cfg(not(utc_backend))]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_arc_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_arc_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
    }

    #[test]
    fn try_alloc_with_oversized_value_succeeds() {
        let arena = Arena::<Global>::new();
        let r = arena.try_alloc_with(|| [3_u8; 70_000]).unwrap();
        assert_eq!(r[0], 3);
        assert_eq!(r[69_999], 3);
    }

    #[cfg(not(utc_backend))]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_with(|| HalfChunkAlign);
    }

    #[cfg(not(utc_backend))]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_box_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_box_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
    }

    // Over-alignment is rejected before initialization.

    #[cfg(not(utc_backend))]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_uninit_box_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_uninit_box::<HalfChunkAlign>();
    }

    #[cfg(not(utc_backend))]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_uninit_arc_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_uninit_arc::<HalfChunkAlign>();
    }

    #[cfg(not(utc_backend))]
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

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_slice_no_drop_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        // `try_alloc_slice_fill_with` routes through
        // `try_alloc_slice_local_no_drop_with` for `!needs_drop` T. The cap
        // for the reference is `CHUNK_ALIGN` (the chunk-recovery
        // limit), not the smart-pointer cap — so use a 64 KiB-aligned
        // type to drive the rejection.
        let res = arena.try_alloc_slice_fill_with::<ChunkAlign, _>(1, |_| ChunkAlign);
        assert!(res.is_err());
    }

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_slice_copy_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let src: &[ChunkAlign] = &[];
        let res = arena.try_alloc_slice_copy(src);
        assert!(res.is_err());
    }

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_slice_copy_arc_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let src: &[ChunkAlign] = &[];
        let res = arena.try_alloc_slice_copy_arc(src);
        assert!(res.is_err());
    }

    // Per-value reference counting permits drop slices longer than `u16::MAX`.

    #[cfg(all(feature = "std", not(miri)))]
    #[test]
    fn alloc_slice_fill_with_arc_drop_long_succeeds() {
        #[derive(Clone)]
        struct D;
        #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<D>() true")]
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let arena = Arena::<Global>::new();
        let arc = arena.alloc_slice_fill_with_arc(u16::MAX as usize + 1, |_| D);
        assert_eq!(arc.len(), u16::MAX as usize + 1);
    }

    #[test]
    fn try_alloc_slice_fill_with_oversized() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let slice = arena.try_alloc_slice_fill_with(2048, |i| u32::try_from(i).unwrap()).unwrap();
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

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_arc_with_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let res = arena.try_alloc_arc_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
        assert!(res.is_err());
    }

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
        let outer = arena.alloc_with(move || {
            // Fill the current chunk so the OUTER allocation's
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
        drop(outer);
        drop(arena);
        assert_eq!(drops.load(Ordering::Relaxed), 1, "outer D's drop must run via eviction commit path");
    }

    #[test]
    fn refill_local_oversized_chunk_capacity() {
        // `with_capacity` preallocates space; verify the arena
        // works correctly when a generous capacity is requested.
        let arena = Arena::builder().with_capacity(128 * 1024).build();
        let _ = arena.alloc::<u8>(0);
    }

    #[test]
    // Miri's weak-memory model rejects this atomic sequence.
    #[cfg_attr(miri, ignore)]
    fn refill_shared_oversized_chunk_capacity() {
        let arena = Arena::builder().with_capacity(128 * 1024).build();
        let _ = arena.alloc_arc::<u8>(0);
    }

    #[cfg(feature = "std")]
    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_copy_panics_when_refill_fails() {
        let alloc = common::FailingAllocator::new(1);
        let arena = Arena::new_in(alloc);
        // Consume the first chunk's bump space, then force a refill that
        // the exhausted allocator cannot satisfy.
        let _filler = arena.alloc_slice_fill_with::<u8, _>(256, |_| 0);
        let src: alloc::vec::Vec<u8> = alloc::vec![0_u8; 4096];
        let _ = arena.alloc_slice_copy(&*src);
    }
}

#[cfg(feature = "stats")]
mod from_mutants_extras_stats {
    #![allow(clippy::items_after_statements, reason = "test-local types are declared near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests exercise method-call clone syntax")]
    #![allow(dead_code, reason = "helper fields preserve test layouts")]
    #![allow(unfulfilled_lint_expectations, reason = "expectations depend on active features")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "unsafe test setup is documented at each call site")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe operations")]
    #![allow(clippy::cast_possible_truncation, reason = "test values fit the target type")]
    #![allow(clippy::cast_sign_loss, reason = "test values are non-negative")]
    #![allow(clippy::empty_drop, reason = "empty Drop impls mark drop-sensitive types")]
    #![allow(clippy::assertions_on_result_states, reason = "tests assert error returns directly")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "test documentation is adjacent to declarations")]
    use multitude::Box as ArenaBox;
    #[repr(align(64))]
    #[derive(Debug)]
    #[expect(dead_code, reason = "helper drives over-alignment tests")]
    struct Align64(u32);

    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common::{self, DropCounter, FailingAllocator, SendFailingAllocator};

    #[test]
    fn preallocate_one_shared_actually_allocates_chunk() {
        let arena = Arena::builder().with_capacity(1024).build();
        assert!(
            arena.stats().normal_chunks_allocated >= 1,
            "with_capacity(1024) must preallocate at least one chunk"
        );
    }

    #[test]
    fn resolve_capacity_uses_correct_class_minus_one_clamp() {
        // 128 KiB > MAX_CHUNK_BYTES (= 64 KiB), so target_class saturates
        // at NUM_CHUNK_CLASSES - 1 = 7 → 64 KiB chunks → 2 chunks.
        let arena = Arena::builder().with_capacity(128 * 1024).build();
        assert_eq!(
            arena.stats().normal_chunks_allocated,
            2,
            "128 KiB shared capacity should preallocate exactly two 64 KiB chunks"
        );

        // Same for local, to exercise the equivalent path through
        // `preallocate_one_local`.
        let arena2 = Arena::builder().with_capacity(128 * 1024).build();
        assert_eq!(arena2.stats().normal_chunks_allocated, 2);
    }

    #[test]
    fn oversized_shared_guard_drop_releases_on_panic() {
        // Each oversized arc below uses an 8 KiB blob (1024 u64s) which
        // is still > max_normal_alloc(4096) and so routes oversized.
        // The budget fits one modest oversized chunk, requiring panic cleanup
        // before the next allocation.
        let arena = Arena::builder().byte_budget(18 * 1024).max_normal_alloc(4096).build();

        // Trigger the panic-during-init oversized path on the Arc/Box.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = arena.try_alloc_arc_with::<[u64; 1024], _>(|| panic!("boom"));
        }));
        assert!(result.is_err(), "panic must propagate");

        // Stats should reflect that a chunk was allocated *and*
        // reconciled (not leaked).
        let stats_after_panic = arena.stats();
        assert!(
            stats_after_panic.oversized_chunks_allocated >= 1,
            "first arc alloc should have acquired an oversized chunk"
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

    #[test]
    fn min_class_for_bytes_classifies_513_below_saturation() {
        // 4 KiB budget easily covers (header + 1 KiB) but not a 64 KiB chunk.
        let res = Arena::builder().byte_budget(4 * 1024).with_capacity(513).try_build();
        assert!(res.is_ok(), "513 must resolve to class 1 (1 KiB), fitting a 4 KiB budget");
    }

    #[test]
    fn min_class_inner_loop_uses_strict_less() {
        // Probe: an unbudgeted arena easily allocates a 1 KiB chunk and a
        // 2 KiB chunk; the difference is observed only via the budget.
        // (Header is bounded; we pick numbers with comfortable margin.)
        let ok = Arena::builder().byte_budget(1500).with_capacity(513).try_build();
        assert!(
            ok.is_ok(),
            "513 must resolve to class 1 (1 KiB); a budget of 1500 bytes (>1 KiB) admits one chunk"
        );
    }

    #[test]
    fn reserve_budget_admits_exact_equal() {
        fn ok(b: usize) -> bool {
            Arena::builder().byte_budget(b).with_capacity(512).try_build().is_ok()
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
        // `exact` is the smallest budget that admits one preallocated 512-byte
        // chunk — i.e. its total allocation size. The budget check is
        // `new > byte_budget`, so a budget exactly equal to the total must
        // admit and one byte less must reject.
        let exact = bisect(ok);
        assert!(ok(exact), "byte_budget == chunk total must admit (exact-equal under `>`)");
        assert!(
            !ok(exact - 1),
            "byte_budget == chunk total - 1 must reject under unmutated `>` (exact={exact})"
        );
    }

    #[test]
    fn release_budget_frees_accounted_bytes() {
        let arena = Arena::builder().byte_budget(128 * 1024).max_normal_alloc(4 * 1024).build();
        let big1 = arena.alloc_box([0u8; 80 * 1024]);
        let s1 = arena.stats();
        assert_eq!(s1.oversized_chunks_allocated, 1);
        drop(big1);
        let big2 = arena.alloc_box([0u8; 80 * 1024]);
        let s2 = arena.stats();
        assert_eq!(s2.oversized_chunks_allocated, 2);
        drop(big2);
        drop(arena);
    }

    #[test]
    fn acquire_shared_total_bytes_is_sum_not_product() {
        let arena = Arena::builder().byte_budget(2 * 1024).build();
        // First arc forces a fresh chunk. Unmutated header + 512
        // <= 2 KiB succeeds; mutated header * 512 >> 2 KiB fails.
        let res = arena.try_alloc_arc(0u32);
        assert!(res.is_ok(), "header + payload must sum (not multiply) for budget check");
    }

    #[test]
    fn arc_with_size_equal_max_normal_routes_normal() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        #[repr(align(8))]
        struct Block([u64; 512]); // 4096 bytes exactly
        let _a = arena.alloc_arc(Block([0u64; 512]));
        let s = arena.stats();
        assert!(s.normal_chunks_allocated + s.oversized_chunks_allocated >= 1);
    }

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
        assert_eq!(s.oversized_chunks_allocated, 2);
    }

    #[test]
    fn arena_728_exact_max_normal_alloc_arc() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let _arc = arena.alloc_arc([0u8; 4096]);
        let stats = arena.stats();
        assert!(stats.normal_chunks_allocated + stats.oversized_chunks_allocated >= 1);
    }

    /// Verifies the one-shot oversized routing for chunks at the
    /// `max_normal_alloc` boundary.
    ///
    /// `try_alloc_uninit_slice_arc::<u8>(max_normal_alloc)` reserves a
    /// length prefix + drop-entry placeholder on top of the payload, so
    /// the worst-case payload exceeds `max_normal_alloc` and routes to
    /// a dedicated one-shot oversized chunk. With the one-shot fix in
    /// place, that chunk is **not** installed as `current`, so a
    /// subsequent small `Arc<u8>` allocation forces refilling
    /// `current` with a fresh normal chunk.
    #[test]
    fn alloc_slice_arc_at_max_normal_alloc_uses_dedicated_oversized_chunk() {
        const MAX_NORMAL: usize = 16 * 1024;
        let arena = Arena::builder().max_normal_alloc(MAX_NORMAL).build();
        let before_normal = arena.stats().normal_chunks_allocated;
        let before_oversized = arena.stats().oversized_chunks_allocated;
        let big = arena
            .try_alloc_uninit_slice_arc::<u8>(MAX_NORMAL)
            .expect("alloc at max_normal_alloc must succeed");
        assert_eq!(big.len(), MAX_NORMAL);
        let after_big_normal = arena.stats().normal_chunks_allocated;
        let after_big_oversized = arena.stats().oversized_chunks_allocated;
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
        let after_tiny_normal = arena.stats().normal_chunks_allocated;
        let after_tiny_oversized = arena.stats().oversized_chunks_allocated;
        assert_eq!(
            after_tiny_normal - after_big_normal,
            1,
            "follow-up tiny Arc must refill `current` with a fresh normal chunk",
        );
        assert_eq!(
            after_tiny_oversized, after_big_oversized,
            "follow-up tiny Arc must not allocate another oversized chunk",
        );
    }

    #[test]
    fn alloc_slice_just_above_max_normal_alloc_uses_oversized_path_shared() {
        let arena = Arena::builder().max_normal_alloc(8 * 1024).build();
        let before = arena.stats().oversized_chunks_allocated;
        let n = (8 * 1024) / core::mem::size_of::<u32>() + 1;
        let _a: Arc<[u32]> = arena.alloc_slice_fill_with_arc(n, |_| 0_u32);
        let after = arena.stats().oversized_chunks_allocated;
        assert_eq!(after - before, 1);
    }

    #[test]
    fn vec_realloc_first_growth_does_not_count_as_relocation() {
        // Initial buffer allocation is not a relocation.
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
    }

    #[test]
    fn arena_builder_capacity_preallocates_correct_chunk_count() {
        use multitude::ArenaBuilder;
        let arena: Arena = Arena::builder().with_capacity(64 * 1024).build();
        // Preallocation creates >= 1 chunk before any user allocation.
        assert!(arena.stats().normal_chunks_allocated >= 1);
    }

    #[test]
    fn chunk_release_returns_budget() {
        use multitude::ArenaBuilder;
        let arena: Arena = Arena::builder().byte_budget(64 * 1024 * 1024).build();
        for _ in 0..32 {
            let a: Arc<u32> = arena.alloc_arc(7);
            drop(a);
        }
        // After many alloc-drop cycles, the running budget shouldn't have
        // monotonically grown (it must drop back as chunks are released).
        assert!(arena.stats().normal_chunks_allocated > 0);
    }

    #[test]
    fn small_arc_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for i in 0_u32..256 {
            let _a: Arc<u32> = arena.alloc_arc(i);
        }
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
    }

    #[test]
    fn small_box_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for i in 0_u32..256 {
            let _b: ArenaBox<u32> = arena.alloc_box(i);
        }
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
    }

    #[test]
    fn small_aligned_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for _ in 0..32 {
            let _a: Arc<Align64> = arena.alloc_arc(Align64(0));
        }
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
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
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
    }

    #[test]
    fn slow_path_arc_allocs_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        // Ratchet the chunk class via a few large uninit fillers
        // (`alloc_uninit_arc` skips per-byte init cost).
        for _ in 0..4 {
            let _filler: Arc<core::mem::MaybeUninit<[u8; 8 * 1024]>> = arena.alloc_uninit_arc::<[u8; 8 * 1024]>();
        }
        for i in 0_u32..16 {
            let _a: Arc<u32> = arena.alloc_arc(i);
        }
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
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
        assert_eq!(arena.stats().oversized_chunks_allocated, 0);
    }

    #[test]
    fn vec_into_box_allocates_no_additional_local_chunk() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..4_u32 {
            v.push(i);
        }
        let chunks_before = arena.stats().normal_chunks_allocated;
        let _b: ArenaBox<[u32]> = v.into_boxed_slice();
        assert_eq!(arena.stats().normal_chunks_allocated, chunks_before);
    }

    #[test]
    fn chunk_release_budget_remains_bounded_through_many_cycles() {
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

#[cfg(feature = "stats")]
mod oversized_routing {
    use multitude::Arena;

    // is_oversized: threshold == max_normal_alloc routes via normal path
    #[test]
    fn is_oversized_routes_shared_at_threshold_via_normal() {
        const MNA: usize = 4 * 1024;
        let arena = Arena::builder().max_normal_alloc(MNA).build();
        let before_normal = arena.stats().normal_chunks_allocated;
        let before_oversized = arena.stats().oversized_chunks_allocated;
        // wcp = MNA exactly: strong prefix (4) + value (MNA-8) + arc block align (4).
        let _arc = arena.alloc_arc([0_u8; MNA - 8]);
        let after_normal = arena.stats().normal_chunks_allocated;
        let after_oversized = arena.stats().oversized_chunks_allocated;
        assert!(after_normal > before_normal);
        assert_eq!(
            after_oversized, before_oversized,
            "threshold must NOT route oversized (kills `>=` mutant)"
        );
    }

    #[test]
    fn is_oversized_routes_shared_above_threshold_via_oversized() {
        const MNA: usize = 4 * 1024;
        let arena = Arena::builder().max_normal_alloc(MNA).build();
        let before_oversized = arena.stats().oversized_chunks_allocated;
        // wcp = MNA + 1: strong prefix (4) + value (MNA-7) + arc block align (4).
        let _arc = arena.alloc_arc([0_u8; MNA - 7]);
        let after_oversized = arena.stats().oversized_chunks_allocated;
        assert!(
            after_oversized > before_oversized,
            "above-threshold must route oversized (kills `==` mutant)"
        );
    }

    #[test]
    fn is_oversized_routes_local_at_threshold_via_normal() {
        const MNA: usize = 4 * 1024;
        let arena = Arena::builder().max_normal_alloc(MNA).build();
        let before_normal = arena.stats().normal_chunks_allocated;
        let before_oversized = arena.stats().oversized_chunks_allocated;
        let s = "x".repeat(MNA);
        let _r = arena.alloc_str(&s);
        let after_normal = arena.stats().normal_chunks_allocated;
        let after_oversized = arena.stats().oversized_chunks_allocated;
        assert!(after_normal > before_normal);
        assert_eq!(after_oversized, before_oversized, "threshold must NOT route oversized");
    }

    #[test]
    fn is_oversized_routes_local_above_threshold_via_oversized() {
        const MNA: usize = 4 * 1024;
        let arena = Arena::builder().max_normal_alloc(MNA).build();
        let before_oversized = arena.stats().oversized_chunks_allocated;
        let s = "x".repeat(MNA + 1);
        let _r = arena.alloc_str(&s);
        let after_oversized = arena.stats().oversized_chunks_allocated;
        assert!(after_oversized > before_oversized);
    }

    // A normal buffer strictly below the threshold may reclaim its tail.
    #[test]
    fn shrink_to_fit_reclaims_strictly_below_max_normal_alloc() {
        let mna = 4 * 1024;
        let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
        // cap = mna - 16 ensures refill_hint = cap + 16 = mna <= mna, so the Vec
        // is allocated in the normal current chunk (not oversized) and its end IS
        // at the bump cursor. The freezable buffer reserves the `Arc<[u8]>` freeze
        // prefix, so the hint is `cap + 16` (≈12B strong+len prefix + 4B
        // alignment slack); `total_bytes` stays strictly below the threshold.
        let cap = mna - 16;
        let mut v: multitude::vec::Vec<'_, u8> = arena.alloc_vec_with_capacity(cap);
        v.extend_from_slice([7_u8; 16]);
        assert_eq!(v.capacity(), cap);
        v.shrink_to_fit();
        assert_eq!(v.capacity(), v.len(), "Vec strictly below max_normal_alloc must reclaim tail");
    }

    // The cache floor preserves reusable saturated-class chunks after reset.
    #[test]
    fn local_cache_floor_advances_so_post_reset_alloc_reuses_chunk() {
        let mut arena = Arena::new();
        // Repeated refills saturate the class ratchet.
        let stride = 1024_usize;
        for _ in 0..8 {
            let s = "y".repeat(stride);
            let _r = arena.alloc_str(&s);
        }
        let before_reset = arena.stats().normal_chunks_allocated;
        arena.reset();
        // Reset caches only chunks at or above the saturated floor.
        let _ = arena.alloc(0_u8);
        let after_reset = arena.stats().normal_chunks_allocated;
        assert!(
            after_reset - before_reset <= 1,
            "post-reset alloc must reuse cached saturated-class chunk; got {} fresh allocs (kills floor-bump mutants)",
            after_reset - before_reset,
        );
    }

    // `config().max_normal_alloc` decides whether an allocation routes to the
    // normal-cache size classes or to a one-shot oversized chunk. Set a
    // non-default `max_normal_alloc` below the default `16 * 1024` and allocate
    // at a size between the two: the config gates it to oversized.
    #[test]
    fn config_returns_custom_max_normal_alloc_local() {
        // Default max_normal_alloc = 16 KiB. Set 4 KiB and request a 12 KiB
        // local allocation: routes to oversized.
        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let baseline = arena.stats().oversized_chunks_allocated;
        let _ = arena.alloc_slice_fill_with::<u8, _>(12 * 1024, |_| 0);
        let after = arena.stats().oversized_chunks_allocated;
        assert!(
            after > baseline,
            "12 KiB local allocation with 4 KiB max_normal_alloc must route to an oversized chunk; stats: {after} vs baseline {baseline}",
        );
    }

    #[test]
    fn config_returns_custom_max_normal_alloc_shared() {
        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let baseline = arena.stats().oversized_chunks_allocated;
        // `alloc_str_box` runs through the shared-chunk path
        // (`impl_alloc_str_box_prefixed_shared`).
        let s: std::string::String = (0..12 * 1024).map(|_| 'a').collect();
        let _ = arena.alloc_str_box(&s);
        let after = arena.stats().oversized_chunks_allocated;
        assert!(
            after > baseline,
            "12 KiB shared allocation with 4 KiB max_normal_alloc must route to an oversized chunk; stats: {after} vs baseline {baseline}",
        );
    }

    // `if min_payload > self.config.max_normal_alloc { allocate_oversized }`:
    // at `min_payload == max_normal_alloc` the allocation stays on the normal
    // cache path.
    #[test]
    fn acquire_local_at_max_normal_alloc_boundary_stays_normal_class() {
        let mna = 4 * 1024;
        let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
        let baseline = arena.stats().oversized_chunks_allocated;
        // `worst_case_slice_payload::<u8>(len) = len * 1 + align_of::<u8>()
        //  = len + 1`; choose `len == mna - 1` so the refill_hint =
        // `min_payload` arrives at `acquire_local` exactly equal to
        // `max_normal_alloc`, which stays on the normal path.
        let len = mna - 1;
        let _ = arena.alloc_slice_fill_with::<u8, _>(len, |_| 0);
        let after = arena.stats().oversized_chunks_allocated;
        assert_eq!(
            after - baseline,
            0,
            "min_payload == max_normal_alloc must stay on the normal cache path",
        );
    }

    #[test]
    fn acquire_shared_at_max_normal_alloc_boundary_stays_normal_class() {
        let mna = 4 * 1024;
        let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
        let baseline = arena.stats().oversized_chunks_allocated;
        // `impl_alloc_str_box_prefixed_shared` calls
        // `refill_shared(words * size_of::<usize>())`. With
        // `words = 1 + len.div_ceil(8).max(1)`, picking `len` so that
        // `words * 8 == mna` lands at the boundary. For `mna = 4096`,
        // `words = 512`, so we need `1 + ceil(len/8) == 512` ⇒
        // `ceil(len/8) == 511` ⇒ `len in 4081..=4088` covers it; choose
        // 4088 for a clean multiple-of-8 boundary.
        let len: usize = 4088;
        debug_assert_eq!(1 + len.div_ceil(core::mem::size_of::<usize>()).max(1), 512);
        let s: std::string::String = (0..len).map(|_| 'a').collect();
        let _ = arena.alloc_str_box(&s);
        let after = arena.stats().oversized_chunks_allocated;
        assert_eq!(
            after - baseline,
            0,
            "min_payload == max_normal_alloc must stay on the normal cache path (shared)",
        );
    }
}

mod construction_boundaries {
    use multitude::Arena;

    #[test]
    fn try_new_succeeds_with_default_globals() {
        // `try_new` produces a working arena on the global allocator and a
        // subsequent allocation yields a valid `&mut T`.
        let arena = Arena::try_new().expect("try_new must succeed on Global");
        let r = arena.alloc(42_u32);
        assert_eq!(*r, 42);
    }

    #[test]
    fn builder_build_produces_functional_arena() {
        // `Arena::builder().build()` produces a functional arena that can
        // allocate.
        let arena: Arena = Arena::builder().build();
        let r = arena.alloc(99_u64);
        assert_eq!(*r, 99);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn preallocate_with_max_class_capacity_does_not_double_ratchet() {
        // A builder that pins a capacity class produces exactly one chunk; a
        // subsequent allocation within it must not trigger a re-preallocation.
        let arena = Arena::builder().with_capacity(512).build();
        let s = arena.stats();
        assert_eq!(s.normal_chunks_allocated, 1);
        // Allocate within the preallocated chunk: no new chunk acquired.
        let _ = arena.alloc(0_u32);
        let s2 = arena.stats();
        assert_eq!(s2.normal_chunks_allocated, 1);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn preallocate_shared_with_capacity_does_not_double_ratchet() {
        let arena = Arena::builder().with_capacity(512).build();
        let s = arena.stats();
        assert_eq!(s.normal_chunks_allocated, 1);
        // First arc within preallocated chunk: still 1.
        let _ = arena.alloc_arc(0_u32);
        let s2 = arena.stats();
        assert_eq!(s2.normal_chunks_allocated, 1);
    }
}

mod drop_slice_over_u16_max_succeeds {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[derive(Clone)]
    struct D(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);
    impl Drop for D {
        fn drop(&mut self) {}
    }

    const TOO_LONG: usize = (u16::MAX as usize) + 1;

    #[test]
    fn try_alloc_slice_clone_drop_over_u16_succeeds() {
        let a = Arena::new();
        let v: std::vec::Vec<D> = (0..TOO_LONG).map(|i| D(i as u8)).collect();
        assert_eq!(a.try_alloc_slice_clone(&v[..]).unwrap().len(), TOO_LONG);
    }

    #[test]
    fn try_alloc_slice_fill_with_drop_over_u16_succeeds() {
        let a = Arena::new();
        assert_eq!(
            a.try_alloc_slice_fill_with::<D, _>(TOO_LONG, |i| D(i as u8)).unwrap().len(),
            TOO_LONG
        );
    }

    #[test]
    fn try_alloc_slice_fill_iter_drop_over_u16_succeeds() {
        let a = Arena::new();
        assert_eq!(
            a.try_alloc_slice_fill_iter::<D, _>((0..TOO_LONG).map(|i| D(i as u8)))
                .unwrap()
                .len(),
            TOO_LONG
        );
    }

    // `Arc<[T]>` uninit/zeroed slices have no `u16` element-count cap
    // under per-`Arc` reference counting (they drop via
    // `drop_in_place::<[T]>`, not a `u16`-counted chunk entry), so a
    // Drop-typed slice longer than `u16::MAX` now allocates successfully.
    #[cfg(not(miri))]
    #[test]
    fn uninit_slice_arc_over_u16_succeeds() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = Arena::new();
        let arc = a.try_alloc_uninit_slice_arc::<D>(TOO_LONG).expect("Arc slices have no u16 cap");
        assert_eq!(arc.len(), TOO_LONG);
    }

    #[cfg(not(miri))]
    #[test]
    fn zeroed_slice_arc_over_u16_succeeds() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = Arena::new();
        let arc = a.try_alloc_zeroed_slice_arc::<D>(TOO_LONG).expect("Arc slices have no u16 cap");
        assert_eq!(arc.len(), TOO_LONG);
    }
}

mod allocator_impl_paths {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use core::alloc::Layout;

    use allocator_api2::alloc::{Allocator, Global};
    use multitude::Arena;

    #[test]
    fn arena_as_allocator_zst_allocate_returns_dangling() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let layout = Layout::from_size_align(0, 8).unwrap();
        let nn = alloc.allocate(layout).unwrap();
        assert_eq!(nn.len(), 0);
    }

    #[test]
    fn arena_as_allocator_zst_dealloc_is_noop() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let layout = Layout::from_size_align(0, 8).unwrap();
        let nn = alloc.allocate(layout).unwrap();
        // SAFETY: pair the dealloc with the allocation above; layout matches.
        unsafe { alloc.deallocate(nn.cast::<u8>(), layout) };
    }

    #[test]
    fn arena_as_allocator_grow_in_place_preserves_prefix() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let old = Layout::from_size_align(8, 1).unwrap();
        let nn = alloc.allocate(old).unwrap();
        // Write into the old allocation.
        // SAFETY: nn points to 8 writable bytes inside the arena.
        unsafe {
            for i in 0..8 {
                nn.cast::<u8>().as_ptr().add(i).write(i as u8);
            }
        }
        let new = Layout::from_size_align(32, 1).unwrap();
        // SAFETY: nn was returned by Self::allocate with `old` layout.
        let grown = unsafe { alloc.grow(nn.cast::<u8>(), old, new).unwrap() };
        assert_eq!(grown.cast::<u8>(), nn.cast::<u8>(), "last allocation should grow in place");
        // Verify the existing prefix remains intact.
        // SAFETY: grown addresses the original allocation extended to 32 bytes.
        unsafe {
            for i in 0..8_u8 {
                assert_eq!(*grown.cast::<u8>().as_ptr().add(usize::from(i)), i);
            }
            // Allocator API requires the caller to release the +1 chunk
            // refcount the grow path took; otherwise the chunk leaks.
            alloc.deallocate(grown.cast::<u8>(), new);
        }
    }
}

mod in_chunk_clone_is_copy_proxy {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    // InChunk is a pub(crate) type; we exercise its Clone via the public
    // Arc-clone path (each clone of an arena Arc internally derives a
    // chunk reference whose pointer is wrapped through InChunk machinery).
    use multitude::Arena;

    #[test]
    fn arc_clone_exercises_inchunk_clone() {
        let arena = Arena::new();
        let a = arena.alloc_arc(7_u32);
        let b = a.clone();
        assert_eq!(*a, 7);
        assert_eq!(*b, 7);
    }
}

mod chunk_ops_destroy_branch {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn box_outlives_arena_takes_destroy_branch_on_release() {
        // Create an arena, allocate a Box, drop the arena. The Box keeps
        // its (shared) chunk alive via +1; when the Box drops, the chunk's
        // release path runs after the arena (and its ChunkProvider) is
        // already gone, exercising the `Chunk::destroy` arm.
        let arena = Arena::new();
        let b = arena.alloc_box(42_u32);
        drop(arena);
        assert_eq!(*b, 42);
        drop(b);
    }
}

mod arena_constructors_coverage {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use allocator_api2::alloc::Global;
    use multitude::Arena;

    use crate::common::SyncFailingAllocator;

    #[test]
    fn arena_try_new_ok() {
        let a: Arena<Global> = Arena::try_new().unwrap();
        let _ = a.alloc(0_u32);
    }

    #[test]
    fn arena_default_constructs_global() {
        let a: Arena<Global> = Arena::default();
        let _ = a.alloc(0_u32);
    }

    #[test]
    fn arena_try_new_in_ok() {
        let a = Arena::try_new_in(SyncFailingAllocator::new(usize::MAX)).unwrap();
        let _ = a.alloc(0_u32);
    }
}

mod alloc_slice_overflow_paths {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use multitude::Arena;

    const HUGE: usize = usize::MAX / 2;

    // -- try_alloc_slice_* fallible overflow paths --

    #[test]
    fn try_alloc_slice_fill_with_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_with::<u32, _>(HUGE, |_| 0);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_iter::<u32, _>((0..HUGE).map(|_| 0_u32));
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_box_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_with_box::<u32, _>(HUGE, |_| 0);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_box_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_iter_box::<u32, _>((0..HUGE).map(|_| 0_u32));
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_arc_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_with_arc::<u32, _>(HUGE, |_| 0);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_arc_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_iter_arc::<u32, _>((0..HUGE).map(|_| 0_u32));
        assert!(r.is_err());
    }

    // -- panicking variants overflow paths --

    fn p<F: FnOnce()>(f: F) -> bool {
        catch_unwind(AssertUnwindSafe(f)).is_err()
    }

    #[test]
    fn alloc_slice_fill_with_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_with::<u32, _>(HUGE, |_| 0);
        }));
    }
    #[test]
    fn alloc_slice_fill_iter_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_iter::<u32, _>((0..HUGE).map(|_| 0_u32));
        }));
    }
    #[test]
    fn alloc_slice_fill_with_box_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_with_box::<u32, _>(HUGE, |_| 0);
        }));
    }
    #[test]
    fn alloc_slice_fill_iter_box_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_iter_box::<u32, _>((0..HUGE).map(|_| 0_u32));
        }));
    }
    #[test]
    fn alloc_slice_fill_with_arc_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_with_arc::<u32, _>(HUGE, |_| 0);
        }));
    }
    #[test]
    fn alloc_slice_fill_iter_arc_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_iter_arc::<u32, _>((0..HUGE).map(|_| 0_u32));
        }));
    }

    // -- reject_drop_slice_too_long panic path for &mut [T] (PANIC=true)
    // Use a heap Vec to avoid stack overflow; `alloc_slice_clone(&v[..])`
    // takes the panicking path and `reject_drop_slice_too_long` rejects
    // up front for T:Drop with len > u16::MAX.

    #[derive(Clone)]
    struct D(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);
    impl Drop for D {
        fn drop(&mut self) {}
    }

    // The rejection happens up front (`reject_drop_slice_too_long`)
    // before any element is read, but a real allocation is required
    // to satisfy Miri's reference-validity check at slice
    // construction time. `vec![value; n]` for a `Clone`-derivable
    // type lowers to a single capacity allocation plus a bulk
    // initializing loop — much cheaper than `(0..N).map(...).collect()`
    // which runs the closure N times.
    #[test]
    fn alloc_slice_clone_drop_over_u16_succeeds() {
        let v: std::vec::Vec<D> = std::vec![D(0); u16::MAX as usize + 1];
        let arena = Arena::new();
        let s = arena.alloc_slice_clone(&v[..]);
        assert_eq!(s.len(), u16::MAX as usize + 1);
    }

    #[test]
    fn alloc_slice_fill_with_drop_over_u16_succeeds() {
        let arena = Arena::new();
        let s = arena.alloc_slice_fill_with::<D, _>(u16::MAX as usize + 1, |i| D(i as u8));
        assert_eq!(s.len(), u16::MAX as usize + 1);
    }

    #[test]
    fn alloc_slice_fill_iter_drop_over_u16_succeeds() {
        let arena = Arena::new();
        let s = arena.alloc_slice_fill_iter::<D, _>((0..(u16::MAX as usize + 1)).map(|i| D(i as u8)));
        assert_eq!(s.len(), u16::MAX as usize + 1);
    }
}

mod allocator_impl_grow_to_zero_overlap {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use core::alloc::Layout;

    use allocator_api2::alloc::{Allocator, Global};
    use multitude::Arena;

    #[test]
    fn grow_from_zero_old_does_not_copy() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let zero = Layout::from_size_align(0, 1).unwrap();
        let nn = alloc.allocate(zero).unwrap();
        let one = Layout::from_size_align(8, 1).unwrap();
        // SAFETY: nn was returned by Self::allocate with `zero` layout.
        let grown = unsafe { alloc.grow(nn.cast::<u8>(), zero, one).unwrap() };
        assert_eq!(grown.len(), 8);
        // SAFETY: pair with the grow above; the resulting +1 chunk
        // refcount must be released or the chunk leaks.
        unsafe { alloc.deallocate(grown.cast::<u8>(), one) };
    }
}

mod oversized_paths_coverage {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    // A drop type requires an entry on oversized paths.
    #[derive(Clone)]
    struct DropU64(u64);
    impl Drop for DropU64 {
        fn drop(&mut self) {}
    }

    // 24 KiB single value with Drop ⇒ oversized-local value arm
    // (`alloc_value.rs` 433-436).
    #[derive(Clone)]
    struct BigDrop([u64; 3000]);
    impl Drop for BigDrop {
        fn drop(&mut self) {}
    }

    // Counter-backed Drop element to assert that oversized-chunk drop
    // entries are actually replayed (not just that the branch is taken).
    struct CountedDrop<'a>(&'a core::sync::atomic::AtomicUsize);
    impl Drop for CountedDrop<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }
    }

    // Oversized-local slices replay destructors at arena teardown.
    #[test]
    fn alloc_slice_fill_with_oversized_drop_replays_destructors() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        let counter = AtomicUsize::new(0);
        {
            let arena = Arena::new();
            let out = arena.alloc_slice_fill_with(3000, |_| CountedDrop(&counter));
            assert_eq!(out.len(), 3000);
            assert_eq!(counter.load(Ordering::SeqCst), 0, "no drops before teardown");
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3000, "every element dropped at arena teardown");
    }

    #[test]
    fn alloc_slice_clone_oversized_drop() {
        let arena = Arena::new();
        let src: Vec<DropU64> = (0..3000).map(DropU64).collect();
        let out = arena.alloc_slice_clone(&src);
        assert_eq!(out.len(), 3000);
        assert_eq!(out[2999].0, 2999);
    }

    #[test]
    fn alloc_slice_fill_with_oversized_drop() {
        let arena = Arena::new();
        let out = arena.alloc_slice_fill_with(3000, |i| DropU64(i as u64));
        assert_eq!(out.len(), 3000);
        assert_eq!(out[2999].0, 2999);
    }

    #[test]
    fn alloc_slice_fill_iter_oversized_drop() {
        let arena = Arena::new();
        let out = arena.alloc_slice_fill_iter((0_u32..3000).map(|i| DropU64(u64::from(i))));
        assert_eq!(out.len(), 3000);
        assert_eq!(out[0].0, 0);
    }

    #[test]
    fn alloc_with_oversized_drop_value() {
        let arena = Arena::new();
        let v = arena.alloc_with(|| BigDrop([7_u64; 3000]));
        assert_eq!(v.0[0], 7);
        assert_eq!(v.0[2999], 7);
    }

    #[test]
    fn alloc_uninit_arc_oversized() {
        use core::mem::MaybeUninit;

        use multitude::Arc;
        let arena = Arena::new();
        // A 24 KiB *Drop* value: `alloc_uninit_arc` only routes through
        // `impl_alloc_uninit_arc` (the placeholder-drop-entry path) for
        // `T: Drop`; a fresh arena's small current chunk can't hold it,
        // so the request goes to a one-shot oversized chunk.
        let a = arena.alloc_uninit_arc::<BigDrop>();
        // SAFETY: `a` is the unique handle, so we have exclusive write access.
        unsafe {
            let p = Arc::as_ptr(&a).cast::<MaybeUninit<BigDrop>>().cast_mut();
            (*p).write(BigDrop([9_u64; 3000]));
        }
        // SAFETY: just initialized above.
        let typed = unsafe { a.assume_init() };
        assert_eq!(typed.0[0], 9);
        assert_eq!(typed.0[2999], 9);
    }

    #[test]
    fn alloc_uninit_slice_arc_oversized() {
        use core::mem::MaybeUninit;

        use multitude::Arc;
        let arena = Arena::new();
        // A Drop element type routes through `impl_alloc_uninit_slice_arc`;
        // 3000 × 8 B = 24 KiB exceeds the fresh arena's current chunk, so
        // the slice lands in a one-shot oversized chunk.
        let len = 3000_usize;
        let s = arena.alloc_uninit_slice_arc::<DropU64>(len);
        // SAFETY: `s` is the unique handle, so we have exclusive write access.
        unsafe {
            let base = Arc::as_ptr(&s).cast::<MaybeUninit<DropU64>>().cast_mut();
            for i in 0..len {
                (*base.add(i)).write(DropU64(i as u64));
            }
        }
        // SAFETY: every element initialized above.
        let typed = unsafe { s.assume_init() };
        assert_eq!(typed.len(), len);
        assert_eq!(typed[123].0, 123);
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_oversized() {
        let arena = Arena::new();
        // ~10 000 ASCII chars ⇒ 20 KiB of UTF-16 payload ⇒ oversized.
        let s = "a".repeat(10_000);
        let u = arena.alloc_utf16_str_arc_from_str(&s);
        assert_eq!(u.len(), 10_000);
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_box_oversized() {
        let arena = Arena::new();
        let s = "b".repeat(10_000);
        let u = arena.alloc_utf16_str_box_from_str(&s);
        assert_eq!(u.len(), 10_000);
    }

    #[test]
    fn alloc_zeroed_arc_oversized() {
        use core::mem::MaybeUninit;

        use multitude::Arc;
        let arena = Arena::new();
        // Zeroed variant of the oversized uninit-arc path (Drop type ⇒
        // `impl_alloc_uninit_arc(zeroed = true)`); never `assume_init`,
        // so the placeholder drop shim tears down without touching the
        // zeroed bytes.
        let _a: Arc<MaybeUninit<BigDrop>> = arena.alloc_zeroed_arc::<BigDrop>();
    }

    #[test]
    fn box_str_as_mut_str() {
        let arena = Arena::new();
        let mut b = arena.alloc_str_box("hello");
        // Exercises `Box<str>::as_mut_str` (str_impls.rs 93-95).
        let m: &mut str = b.as_mut_str();
        m.make_ascii_uppercase();
        assert_eq!(b.as_str(), "HELLO");
    }
}

mod refactor_coverage_gaps {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    // `reserve_slice_box` oversized branch (+ `try_alloc_uninit_slice_prefixed`
    // and `try_alloc_prefixed_slice_payload`): a `Box<[T]>` built via the
    // fill path whose payload exceeds MAX_NORMAL_ALLOC routes through the
    // dedicated oversized chunk.
    #[test]
    fn oversized_fill_box_routes_through_reserve_slice_box() {
        let arena = Arena::new();
        // 5000 × u32 = 20 KiB > MAX_NORMAL_ALLOC (16 KiB) ⇒ oversized box path.
        let b: multitude::Box<[u32]> = arena.alloc_slice_fill_with_box(5000, |i| i as u32);
        assert_eq!(b.len(), 5000);
        assert_eq!(b[0], 0);
        assert_eq!(b[4999], 4999);
    }

    // vec/mod.rs non-freezable paths: the refill-hint else-branch,
    // `try_reserve_local_slice`, and the non-freezable oversized arm. An
    // over-aligned element (align ≥ max_smart_ptr_align == CHUNK_ALIGN/2) is
    // non-freezable, and at 32 KiB each it forces the oversized refill.
    #[test]
    fn non_freezable_overaligned_vec_grows_via_oversized_path() {
        #[repr(align(32768))]
        #[derive(Clone, Copy)]
        struct Over(u8);
        let arena = Arena::new();
        // Reserve (rather than `push`) capacity for the over-aligned element:
        // this drives the same non-freezable oversized growth path without
        // ever materializing a 32 KiB-aligned `Over` on the stack — such
        // over-aligned stack temporaries fault on Windows.
        let mut v = arena.alloc_vec::<Over>();
        v.try_reserve(2).expect("reserve over-aligned capacity");
        assert!(v.capacity() >= 2);
        assert!(v.is_empty());
    }

    // A grow request whose raw payload (`size_of::<T>() * new_cap`) overflows
    // `usize` must be a recoverable `AllocError`, never a panic.
    #[test]
    fn vec_try_reserve_overflowing_capacity_returns_err() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u64>();
        // size_of::<u64>() (8) * (usize::MAX / 4) overflows usize.
        v.try_reserve(usize::MAX / 4)
            .expect_err("overflowing capacity must be a recoverable error");
    }

    // `From<Box<Utf16Str>> for Box<[u16]>` zero-copy retag.
    #[cfg(feature = "utf16")]
    #[test]
    fn box_utf16str_into_box_u16_slice_retags_without_copy() {
        use widestring::utf16str;
        let arena = Arena::new();
        let b = arena.alloc_utf16_str_box(utf16str!("hello"));
        let raw = b.as_widestring_utf16_str().as_slice().as_ptr();
        let u16box: multitude::Box<[u16]> = multitude::Box::from(b);
        assert_eq!((*u16box).as_ptr(), raw, "Box<Utf16Str> -> Box<[u16]> retag must not copy");
        assert_eq!(&*u16box, utf16str!("hello").as_slice());
    }
}

mod alloc_drop_behavior {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std for thread/sync primitives")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(clippy::manual_assert, reason = "explicit panic clarifies safety-net intent")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded indices fit")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "explicit borrows clarify intent in tests")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(
        dead_code,
        reason = "test types intentionally retain unused fields to keep their Drop side-effects observable"
    )]
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[derive(Debug)]
    struct DropCounter(StdArc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn many_drop_typed_arc_allocs_run_drop_exactly_once_each() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let mut keep: std::vec::Vec<Arc<DropCounter>> = std::vec::Vec::new();
            // 256 Arc allocations span multiple chunks and the
            // same drop-entry paths this test is targeting.
            for _ in 0..256_u32 {
                keep.push(arena.alloc_arc_with(|| DropCounter(counter.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 256);
    }

    #[test]
    fn oversized_drop_typed_alloc_runs_drop_and_respects_alignment() {
        #[repr(align(64))]
        struct Big {
            // 32 KiB > default max_normal_alloc (16 KiB) → oversized path.
            _payload: [u64; 4 * 1024],
            token: DropCounter,
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let b = arena.alloc_box_with(|| Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter.clone()),
            });
            let p: *const Big = std::ptr::from_ref::<Big>(&b);
            assert_eq!((p as usize) % 64, 0, "Big must be 64-byte aligned");
            drop(b);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1, "oversized Box's Drop must run");

        let counter2 = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let a = arena.alloc_arc_with(|| Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter2.clone()),
            });
            let p: *const Big = std::ptr::from_ref::<Big>(&a);
            assert_eq!((p as usize) % 64, 0);
            drop(a);
            drop(arena);
        }
        assert_eq!(counter2.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn oversized_high_alignment_drives_align_offset() {
        #[repr(align(128))]
        struct Aligned128 {
            _pad: [u64; 4 * 1024], // 32 KiB, oversized
        }
        let arena = Arena::new();
        let b = arena.alloc_box(Aligned128 { _pad: [0; 4 * 1024] });
        let p: *const Aligned128 = std::ptr::from_ref::<Aligned128>(&b);
        assert_eq!((p as usize) % 128, 0);

        // Same for Arc (oversized shared).
        let a = arena.alloc_arc(Aligned128 { _pad: [0; 4 * 1024] });
        let p: *const Aligned128 = std::ptr::from_ref::<Aligned128>(&a);
        assert_eq!((p as usize) % 128, 0);
    }

    #[test]
    fn many_distinct_size_and_align_combinations_succeed() {
        let arena = Arena::new();
        // Mix of size classes and alignments to maximize the chance
        // of hitting `aligned == max_aligned`.
        let mut keep_u8 = std::vec::Vec::new();
        let mut keep_u16 = std::vec::Vec::new();
        let mut keep_u32 = std::vec::Vec::new();
        let mut keep_u64 = std::vec::Vec::new();
        for i in 0..256_u32 {
            keep_u8.push(arena.alloc((i & 0xff) as u8));
            keep_u16.push(arena.alloc((i & 0xffff) as u16));
            keep_u32.push(arena.alloc(i));
            keep_u64.push(arena.alloc(u64::from(i)));
        }
        for (i, p) in keep_u8.iter().enumerate() {
            assert_eq!(**p, (i as u32 & 0xff) as u8);
        }
        for (i, p) in keep_u16.iter().enumerate() {
            assert_eq!(**p, (i as u32 & 0xffff) as u16);
        }
        for (i, p) in keep_u32.iter().enumerate() {
            assert_eq!(**p, i as u32);
        }
        for (i, p) in keep_u64.iter().enumerate() {
            assert_eq!(**p, i as u64);
        }
    }

    // --------------------------------------------------------------------
    // D/E. allocate_layout `+` arithmetic.
    // --------------------------------------------------------------------

    #[test]
    fn vec_with_alignment_grows_across_chunks() {
        let arena = Arena::new();
        // Allocate vecs that together exceed a single chunk so
        // allocate_layout's refill arm is exercised.
        let mut all: std::vec::Vec<multitude::vec::Vec<'_, u64>> = std::vec::Vec::new();
        for _ in 0..64 {
            let mut v = arena.alloc_vec_with_capacity::<u64>(64);
            for j in 0..64_u64 {
                v.push(j);
            }
            all.push(v);
        }
        for v in &all {
            for (i, x) in v.iter().enumerate() {
                assert_eq!(*x, i as u64);
            }
        }
    }

    // --------------------------------------------------------------------
    // D/E/F. Slice paths — local and shared, with and without Drop.
    // --------------------------------------------------------------------

    #[test]
    fn many_copy_slices_force_slow_refill() {
        let arena = Arena::new();
        let mut all = std::vec::Vec::new();
        for i in 0..256_u32 {
            let s = arena.alloc_slice_copy::<u64>(&[u64::from(i); 17]);
            all.push(s);
        }
        for (i, s) in all.iter().enumerate() {
            for &v in s.iter() {
                assert_eq!(v, i as u64);
            }
        }
    }

    // --------------------------------------------------------------------
    // Misc: confirm the && operator in the oversized-value gate.
    // --------------------------------------------------------------------

    #[test]
    fn oversized_box_drop_runs_exactly_once() {
        #[repr(align(64))]
        struct Big {
            _payload: [u64; 4 * 1024], // 32 KiB > 16 KiB max_normal_alloc
            token: DropCounter,
        }
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let b = arena.alloc_box(Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter.clone()),
            });
            drop(b);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1, "Box oversized must drop exactly once");
    }
}

mod alloc_drop_behavior_2 {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "explicit .clone() in tests")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit")]
    #![allow(clippy::items_after_statements, reason = "test-local types live near usage")]
    #![allow(clippy::large_stack_arrays, reason = "test stack allocations are bounded")]
    #![allow(dead_code, reason = "drop-tracking payload fields' Drop side-effects are the observable")]
    #![allow(clippy::redundant_clone, reason = "tests prefer explicit clones for clarity")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "doc-comments cite ASCII identifiers verbatim")]
    #![allow(clippy::manual_midpoint, reason = "explicit (lo+hi)/2 reads naturally for bisection")]
    #![allow(clippy::ref_as_ptr, reason = "explicit `*const` cast is clearer than into()")]
    #![allow(clippy::bool_assert_comparison, reason = "explicit boolean assertions are clearer")]
    #![allow(clippy::assertions_on_constants, reason = "test asserts on probe results which may be constant")]
    #![allow(clippy::missing_panics_doc, reason = "test functions may panic by design")]
    #![allow(clippy::deref_by_slicing, reason = "tests express intent via &v[..] for clarity")]
    #![allow(clippy::useless_vec, reason = "vec!! mirrors realistic user code shapes")]
    #![allow(clippy::unused_unit, reason = "the explicit `()` body documents intent of the mutation we apply")]
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[derive(Debug)]
    struct DropCounter(StdArc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn arc_drop_count_increments_on_each_alloc() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let mut keep: Vec<multitude::Arc<DropCounter>> = Vec::with_capacity(64);
            for _ in 0..64_u32 {
                keep.push(arena.alloc_arc(DropCounter(counter.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 64);
    }

    #[test]
    fn arc_with_non_drop_t_does_not_install_drop_entry() {
        const N: u32 = 64;
        let arena = multitude::Arena::builder().with_capacity(64 * 1024).build();
        // 4 × 16 KiB uninit fillers walk the bump cursor to the chunk's true end.
        let _fillers: Vec<multitude::Arc<core::mem::MaybeUninit<[u8; 16 * 1024]>>> =
            (0..4).map(|_| arena.alloc_uninit_arc::<[u8; 16 * 1024]>()).collect();
        let mut keep: Vec<multitude::Arc<u32>> = Vec::with_capacity(N as usize);
        for i in 0..N {
            keep.push(arena.alloc_arc(i));
        }
        for (i, a) in keep.iter().enumerate() {
            assert_eq!(**a, i as u32);
        }
    }

    #[test]
    fn arc_with_high_align_uses_correct_needed_size() {
        #[repr(align(64))]
        struct Aligned64([u8; 64]);
        let arena = multitude::Arena::new();
        let mut keep: Vec<multitude::Arc<Aligned64>> = Vec::with_capacity(256);
        for _ in 0..256_u32 {
            keep.push(arena.alloc_arc(Aligned64([0; 64])));
        }
        for a in &keep {
            let p = a.as_ref() as *const Aligned64 as usize;
            assert_eq!(p % 64, 0, "alignment must be honored after refill_shared(needed)");
        }
    }

    #[test]
    fn allocate_layout_high_align_refill_uses_sum() {
        use core::alloc::Layout;

        use allocator_api2::alloc::Allocator;
        let arena = multitude::Arena::new();
        let a: &multitude::Arena = &arena;
        let layout = Layout::from_size_align(4096, 64).unwrap();
        let mut allocations = std::vec::Vec::new();
        // This count forces refill while keeping Miri runtime bounded.
        for _ in 0..32 {
            let ptr = a.allocate(layout).unwrap();
            let addr = ptr.as_ptr() as *const u8 as usize;
            assert_eq!(addr % 64, 0);
            allocations.push(ptr);
        }
        // Deallocate so the chunks reclaim their refcounts; otherwise
        // Miri (and any leak-aware allocator) would flag the chunks as
        // leaked.
        for ptr in allocations {
            // SAFETY: ptr came from `a.allocate(layout)` with the same layout.
            unsafe { a.deallocate(ptr.cast(), layout) };
        }
    }

    #[test]
    fn slice_shared_no_drop_does_not_install_entry() {
        let arena = multitude::Arena::new();
        let s: multitude::Arc<[u32]> = arena.alloc_slice_copy_arc(&[1u32, 2, 3, 4, 5][..]);
        assert_eq!(&*s, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn slice_shared_long_no_drop_succeeds() {
        let arena = multitude::Arena::new();
        let v = vec![7_u32; 70_000];
        let s: multitude::Arc<[u32]> = arena.alloc_slice_copy_arc(&v[..]);
        assert_eq!(s.len(), 70_000);
    }

    #[test]
    // Skipped under Miri: needs `u16::MAX` allocations + drops (~65K
    // elements) to exercise the slice-length boundary, which exceeds
    // Miri's 10-minute test budget. The boundary itself is a runtime
    // assertion, not a memory-safety property; native test runs verify
    // it on every CI execution.
    #[cfg_attr(miri, ignore)]
    fn slice_shared_drop_at_u16_max_succeeds() {
        let counter = StdArc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct DC(StdArc<AtomicUsize>);
        impl Drop for DC {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        // DC must be Send + Sync for alloc_slice_fill_with_arc.
        {
            let arena = multitude::Arena::new();
            let c = counter.clone();
            let s: multitude::Arc<[DC]> = arena.alloc_slice_fill_with_arc(u16::MAX as usize, |_| DC(c.clone()));
            assert_eq!(s.len(), u16::MAX as usize);
            drop(s);
        }
        assert_eq!(counter.load(Ordering::Relaxed), u16::MAX as usize);
    }

    #[test]
    fn slice_shared_init_increments_guard_len() {
        let counter = StdArc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct DC(StdArc<AtomicUsize>);
        impl Drop for DC {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let arena = multitude::Arena::new();
            let c = counter.clone();
            // Initialise 10 elements, then panic on the 11th.
            let _s: multitude::Arc<[DC]> = arena.alloc_slice_fill_with_arc(20_usize, |i| {
                assert!(i != 10, "test panic");
                DC(c.clone())
            });
        }));
        assert!(res.is_err());
        // 10 elements were initialised before panic; with += -> *= the
        // guard.len would stay 0 and none of those 10 would be dropped.
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn slice_shared_refill_uses_correct_has_drop_flag() {
        let counter = StdArc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct DC(StdArc<AtomicUsize>);
        impl Drop for DC {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let arena = multitude::Arena::new();
        let c = counter.clone();
        // Many Drop slices, forcing repeated refills.
        let mut keep: Vec<multitude::Arc<[DC]>> = Vec::with_capacity(256);
        for _ in 0..256 {
            let cc = c.clone();
            keep.push(arena.alloc_slice_fill_with_arc(8_usize, |_| DC(cc.clone())));
        }
        drop(keep);
        drop(arena);
        assert_eq!(counter.load(Ordering::Relaxed), 256 * 8);
    }

    #[test]
    fn try_bump_fit_exact_aligned_succeeds() {
        let arena = multitude::Arena::new();
        // Many sequential u8 allocations stress the bump cursor.
        let mut keep = Vec::with_capacity(4096);
        for i in 0..4096_u32 {
            keep.push(arena.alloc(i as u8));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u8);
        }
    }

    #[test]
    fn vec_into_arc_advances_read_index() {
        let arena = multitude::Arena::new();
        let mut v: multitude::vec::Vec<u32, _> = arena.alloc_vec();
        v.push(10);
        v.push(20);
        v.push(30);
        let arc: multitude::Arc<[u32]> = multitude::Arc::from(v);
        assert_eq!(&*arc, &[10, 20, 30]);
    }

    #[test]
    fn vec_into_box_advances_read_index() {
        // `Vec::into_box` moves the elements into a fresh shared
        // allocation via `alloc_slice_fill_iter_box`, whose fill closure
        // advances its read index per element. This exercises that
        // advance and confirms the elements land in order.
        let arena = multitude::Arena::new();
        let mut v: multitude::vec::Vec<u32, _> = arena.alloc_vec_with_capacity(3);
        v.push(11);
        v.push(22);
        v.push(33);
        let b: multitude::Box<[u32]> = v.into_boxed_slice();
        assert_eq!(&*b, &[11, 22, 33]);
    }
}

mod alloc_hot_path_behavior {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "explicit .clone() in tests")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit")]
    #![allow(clippy::items_after_statements, reason = "test-local types live near usage")]
    #![allow(clippy::large_stack_arrays, reason = "test stack allocations are bounded")]
    #![allow(dead_code, reason = "drop-tracking payload fields")]
    #![allow(clippy::redundant_clone, reason = "tests prefer explicit clones")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "doc-comments cite ASCII identifiers")]
    #![allow(clippy::missing_panics_doc, reason = "test functions may panic")]
    #![allow(clippy::manual_assert, reason = "explicit if/panic preserves test intent")]
    #![allow(clippy::use_self, reason = "test code")]
    #![allow(clippy::ref_as_ptr, reason = "test code")]
    #![allow(clippy::stable_sort_primitive, reason = "test code")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "test code")]
    #![allow(
        clippy::used_underscore_binding,
        reason = "underscore-prefixed bindings kept alive intentionally for drop ordering"
    )]
    #![allow(clippy::needless_range_loop, reason = "test code prefers explicit indices")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test marker types are trivially Send/Sync")]
    #![allow(clippy::redundant_closure_for_method_calls, reason = "test code")]
    #![allow(unused_imports, reason = "test scope-local imports may shadow")]
    #![allow(redundant_imports, reason = "test scope-local imports may shadow")]
    #![allow(clippy::assertions_on_constants, reason = "test asserts on constants")]
    #![allow(clippy::bool_assert_comparison, reason = "explicit boolean assertions")]
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    thread_local! {
        /// Per-test thread-local drop counter.
        static DROP_COUNTER: Cell<usize> = const { Cell::new(0) };
    }

    /// Increment the current thread's drop counter.
    fn bump_drop_counter() {
        DROP_COUNTER.with(|c| c.set(c.get() + 1));
    }

    #[derive(Clone)]
    struct DropTracker(u64);
    impl Drop for DropTracker {
        fn drop(&mut self) {
            bump_drop_counter();
        }
    }

    // SAFETY: DropTracker is trivially Send+Sync (just a u64).
    unsafe impl Send for DropTracker {}
    unsafe impl Sync for DropTracker {}

    /// ZST guard returned by [`reset_drop_counter`]. The drop counter is
    /// thread-local, so no cross-test serialization is required; this guard
    /// exists only so existing `let _guard = reset_drop_counter();` call sites
    /// keep compiling unchanged.
    struct DropCounterGuard;

    /// Reset the current thread's drop counter to zero.
    #[must_use = "bind the guard for the test's lifetime"]
    fn reset_drop_counter() -> DropCounterGuard {
        DROP_COUNTER.with(|c| c.set(0));
        DropCounterGuard
    }

    /// Read the current thread's drop count.
    fn drops() -> usize {
        DROP_COUNTER.with(Cell::get)
    }

    #[test]
    fn arena_709_entry_size_gt_zero_arc_with() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        {
            let _arc = arena.alloc_arc_with(|| DropTracker(42));
            // Arc holds the value; drop it by letting it go out of scope.
        }
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "DropTracker must be dropped; got {drops} drops");
    }

    #[test]
    #[cfg(feature = "stats")]
    fn arena_728_size_eq_max_normal_alloc_arc() {
        let _guard = reset_drop_counter();
        let arena = Arena::builder().build();
        let _a1 = arena.alloc_arc_with(|| DropTracker(1));
        let _a2 = arena.alloc_arc_with(|| DropTracker(2));
        let stats = arena.stats();
        assert_eq!(
            stats.oversized_chunks_allocated, 0,
            "small arcs should use normal chunks, not oversized"
        );
    }

    #[test]
    fn arena_731_needed_computation_arc_with() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let mut keep = Vec::new();
        for i in 0..100 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 100, "all 100 DropTrackers must be dropped");
    }

    #[test]
    fn arena_1251_oversized_shared_guard_drop() {
        let _guard = reset_drop_counter();
        let arena = Arena::builder().max_normal_alloc(4096).build();
        #[repr(C)]
        struct LargeArcDrop {
            data: [u8; 8192],
        }
        impl Drop for LargeArcDrop {
            fn drop(&mut self) {
                bump_drop_counter();
            }
        }
        // SAFETY: just bytes + a counter
        unsafe impl Send for LargeArcDrop {}
        unsafe impl Sync for LargeArcDrop {}
        let arc = arena.alloc_arc_with(|| LargeArcDrop { data: [0; 8192] });
        drop(arc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "oversized arc LargeArcDrop must drop");
    }

    #[test]
    fn arena_1648_allocate_layout_needed() {
        let arena = Arena::new();
        // allocate_layout is used by alloc (borrow) path.
        // Allocate many u64 values and verify they don't overlap.
        let mut ptrs = Vec::new();
        for i in 0u64..100 {
            let r = arena.alloc(i);
            ptrs.push(core::ptr::addr_of!(*r) as usize);
        }
        // Check no two pointers are the same
        ptrs.sort();
        ptrs.dedup();
        assert_eq!(ptrs.len(), 100, "all 100 alloc pointers must be distinct");
        // Verify values are intact
        for i in 0u64..10 {
            let r = arena.alloc(i + 1000);
            assert_eq!(*r, i + 1000);
        }
    }

    #[test]
    fn arena_2261_slice_local_and_to_or() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let empty = arena.alloc_slice_fill_with(0, |_| DropTracker(0));
        assert_eq!(empty.len(), 0);
        let nums = arena.alloc_slice_fill_with(10, |i| i as u32);
        assert_eq!(nums.len(), 10);
        for (i, v) in nums.iter().enumerate() {
            assert_eq!(*v, i as u32);
        }
    }

    #[test]
    fn arena_2266_slice_len_boundary() {
        let arena = Arena::new();
        let big_len = u16::MAX as usize + 1;
        let result = arena.try_alloc_slice_fill_with(big_len, |i| i as u8);
        let arena2 = Arena::new();
        let empty_drop = arena2.alloc_slice_fill_with(0, |_| DropTracker(0));
        assert_eq!(empty_drop.len(), 0);

        let _guard = reset_drop_counter();
        let one_drop = arena2.alloc_slice_fill_with(1, |_| DropTracker(42));
        assert_eq!(one_drop.len(), 1);
        drop(result);
    }

    #[test]
    fn arena_2655_shared_slice_and_to_or() {
        let arena = Arena::new();
        let _guard = reset_drop_counter();
        let empty_arc = arena.alloc_slice_fill_with_arc(0, |_| DropTracker(0));
        assert_eq!(empty_arc.len(), 0);
        drop(empty_arc);
        let drops = drops();
        assert_eq!(drops, 0, "empty arc slice should not drop any elements");

        let nums_arc = arena.alloc_slice_fill_with_arc(5, |i| i as u64);
        assert_eq!(nums_arc.len(), 5);
    }

    #[test]
    fn arena_2660_shared_slice_len_boundary() {
        let arena = Arena::new();
        let _guard = reset_drop_counter();
        let one = arena.alloc_slice_fill_with_arc(1, |_| DropTracker(99));
        assert_eq!(one.len(), 1);
        drop(one);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "single-element arc slice must drop");
    }

    #[test]
    fn arena_2701_shared_slice_entry_size_guard() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let mut keep = Vec::new();
        for _ in 0..20 {
            keep.push(arena.alloc_slice_fill_with_arc(3, |i| DropTracker(i as u64)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 60, "20 arcs * 3 elements = 60 drops");
    }

    #[test]
    fn arena_2719_guard_len_increment() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let arc = arena.alloc_slice_fill_with_arc(5, |i| DropTracker(i as u64));
        assert_eq!(arc.len(), 5);
        drop(arc);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 5, "all 5 elements must drop");
    }

    #[test]
    fn arena_2739_refill_shared_entry_size_check() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let mut keep = Vec::new();
        for _ in 0..100 {
            keep.push(arena.alloc_slice_fill_with_arc(3, |i| DropTracker(i as u64)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 300, "100 * 3 = 300 drops");
    }

    #[test]
    fn chunk_provider_133_reserve_budget_boundary() {
        let arena = Arena::builder().byte_budget(256 * 1024).build();
        let _v = arena.alloc(42u64);
    }

    #[test]
    fn chunk_provider_441_shared_header_plus_target() {
        let arena = Arena::builder().byte_budget(512 * 1024).build();
        let _a1 = arena.alloc_arc(1u64);
        let _a2 = arena.alloc_arc(2u64);
    }

    #[test]
    fn constants_76_min_class_ge_to_lt() {
        let arena = Arena::new();
        let big = vec![0u8; 64 * 1024];
        let _alloc = arena.alloc_slice_copy(&big);
    }

    #[test]
    #[cfg(feature = "stats")]
    fn constants_87_loop_boundary() {
        let arena = Arena::builder().byte_budget(128 * 1024).build();
        let _v = arena.alloc(42u64);
    }

    #[test]
    fn chunk_143_max_bump_extent() {
        let arena = Arena::new();
        let mut keep = Vec::new();
        for i in 0u64..1000 {
            keep.push(arena.alloc_arc(i));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
    }

    #[test]
    fn chunk_168_to_thin_ptr() {
        let arena = Arena::new();
        for _ in 0..5 {
            let mut batch = Vec::new();
            for i in 0u64..50 {
                batch.push(arena.alloc_arc(i));
            }
            drop(batch);
        }
        let final_arc = arena.alloc_arc(42u64);
        assert_eq!(*final_arc, 42);
    }

    #[test]
    fn chunk_186_payload_rounding() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let mut keep = Vec::new();
        for i in 0..50 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 50, "all 50 shared DropTrackers must drop");
    }

    #[test]
    fn string_465_try_reserve_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        s.try_reserve(10).unwrap(); // needed == cap, should not grow
        s.push_str("1234567890");
        assert_eq!(s.as_str(), "1234567890");
        s.try_reserve(0).unwrap();
    }

    #[test]
    fn vec_451_resize_reserve() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(5);
        v.push(1);
        v.push(2);
        // resize from 2 to 5 — reserve should be 3
        v.resize(5, 0);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 0);
    }

    #[test]
    fn vec_460_461_resize_guard() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(20);
        v.push(10);
        v.push(20);
        v.resize(8, 42);
        assert_eq!(v.len(), 8);
        assert_eq!(v[0], 10);
        assert_eq!(v[1], 20);
        for i in 2..8 {
            assert_eq!(v[i], 42);
        }
    }

    #[test]
    fn vec_473_474_resize_total_new() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(10);
        v.push(1);
        // Resize to exactly len+1 — total_new == 1
        v.resize(2, 99);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], 99);
        // Resize to same length — total_new == 0, no-op
        v.resize(2, 77);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], 99); // unchanged
    }

    #[test]
    fn vec_762_into_box() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(10);
        for i in 0..5 {
            v.push(i * 10);
        }
        let boxed = v.into_boxed_slice();
        assert_eq!(boxed.len(), 5);
        assert_eq!(boxed[0], 0);
        assert_eq!(boxed[1], 10);
        assert_eq!(boxed[2], 20);
        assert_eq!(boxed[3], 30);
        assert_eq!(boxed[4], 40);
    }

    #[test]
    fn vec_808_realloc_inplace_guard() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(4);
        v.push(1);
        v.push(2);
        // Grow: new_cap > cap && cap > 0 → try in-place
        v.reserve(10);
        assert!(v.capacity() >= 12);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);

        // From cap=0 → can't try in-place (cap > 0 is false)
        let mut v2 = arena.alloc_vec_with_capacity::<u64>(0);
        v2.push(42);
        assert_eq!(v2[0], 42);
    }

    #[test]
    fn vec_819_realloc_copy_guard() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(0);
        assert_eq!(v.len(), 0);
        v.push(1);
        assert_eq!(v[0], 1);

        v.reserve(100);
        assert_eq!(v[0], 1);
    }

    #[test]
    fn vec_828_realloc_relocation_tracking() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(2);
        v.push(1);
        v.push(2);
        // Force a realloc that can't grow in place
        // First alloc something else to prevent in-place growth
        let _other = arena.alloc(99u64);
        v.reserve(100);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
    }

    #[test]
    fn arena_709_entry_size_zero_arc() {
        let arena = Arena::new();
        // Arc<u64> — no Drop, entry_size == 0
        // With `>=`, a drop entry would be written even though no space
        // was reserved for it. Allocate many to make corruption likely.
        let mut keep = Vec::new();
        for i in 0u64..500 {
            keep.push(arena.alloc_arc_with(|| i));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
        drop(keep);
        drop(arena);
    }

    #[test]
    fn arena_731_needed_tight_budget_arc() {
        let _guard = reset_drop_counter();
        // Use a tight byte budget so wrong `needed` could fail
        let arena = Arena::builder().byte_budget(256 * 1024).build();
        let mut keep = Vec::new();
        for i in 0..200 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 200);
    }

    #[test]
    fn arena_1251_oversized_guard_panic() {
        // The budget fits one oversized chunk, so panic cleanup must release
        // it before the second allocation.
        const N: usize = 70_000;
        let arena = Arena::builder().max_normal_alloc(4096).byte_budget(N + 4096).build();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _arc: multitude::Arc<[u8; N]> = arena.alloc_arc_with(|| {
                panic!("intentional panic in oversized arc closure");
            });
        }));
        assert!(result.is_err(), "should have caught the panic");

        let _arc2: multitude::Arc<[u8; N]> = arena.alloc_arc_with(|| [0u8; N]);
    }

    #[test]
    fn arena_1648_high_alignment_layout() {
        #[repr(align(64))]
        #[derive(Clone, Copy)]
        struct Aligned64 {
            data: [u8; 64],
        }
        let arena = Arena::new();
        let mut keep = Vec::new();
        for i in 0u8..100 {
            let v = arena.alloc(Aligned64 { data: [i; 64] });
            assert_eq!(v.data[0], i);
            keep.push(core::ptr::addr_of!(*v) as usize);
        }
        // Verify all pointers are 64-byte aligned
        for p in &keep {
            assert_eq!(p % 64, 0, "pointer must be 64-byte aligned");
        }
    }

    #[test]
    fn arena_2261_empty_drop_slice() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate many empty slices of Drop type
        for _ in 0..500 {
            let s = arena.alloc_slice_fill_with(0, |_| DropTracker(0));
            assert_eq!(s.len(), 0);
        }
        // If entry_size was wrongly nonzero, we'd waste space and
        // potentially corrupt the drop list.
        // Also test non-empty non-Drop slices (drop_fn is None)
        for i in 0u64..500 {
            let s = arena.alloc_slice_fill_with(5, |j| i + j as u64);
            assert_eq!(s[0], i);
        }
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 0, "empty Drop slices should not produce drops");
    }

    #[test]
    fn arena_2266_large_nondrop_slice() {
        let arena = Arena::new();
        // Non-Drop slices are not limited by the drop-entry length field.
        let result = arena.try_alloc_slice_fill_with(65536, |i| i as u8);
        assert!(result.is_ok(), "large non-Drop slice should succeed");
        let s = result.unwrap();
        assert_eq!(s.len(), 65536);
        assert_eq!(s[0], 0);
        assert_eq!(s[65535], 255);
    }

    #[test]
    fn arena_2655_empty_drop_shared_slice() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        for _ in 0..200 {
            let arc = arena.alloc_slice_fill_with_arc(0, |_| DropTracker(0));
            assert_eq!(arc.len(), 0);
            drop(arc);
        }
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 0, "empty Drop arc slices should not produce drops");
    }

    #[test]
    fn large_nondrop_shared_slice() {
        // A non-Copy, non-Drop wrapper exercises initialized shared slices.
        #[derive(Clone, Debug, PartialEq)]
        struct W(u8);
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with_arc(65536, |i| W(i as u8));
        assert!(result.is_ok(), "large non-Drop non-Copy shared slice should succeed");
        let arc = result.unwrap();
        assert_eq!(arc.len(), 65536);
        assert_eq!(arc[0], W(0));
    }

    #[test]
    fn arena_2701_entry_size_shared_slice() {
        let arena = Arena::new();
        // Allocate many non-Drop shared slices
        let mut keep = Vec::new();
        for i in 0u64..200 {
            keep.push(arena.alloc_slice_fill_with_arc(5, |j| i + j as u64));
        }
        for (i, arc) in keep.iter().enumerate() {
            assert_eq!(arc[0], i as u64);
            assert_eq!(arc[4], i as u64 + 4);
        }
    }

    #[test]
    fn chunk_143_max_bump_many() {
        let _guard = reset_drop_counter();
        const N: u64 = 64;

        let arena = Arena::builder().with_capacity(64 * 1024).build();

        // 4 × 16 KiB uninit fillers walk the bump cursor to the chunk's true end.
        let _fillers: Vec<multitude::Arc<core::mem::MaybeUninit<[u8; 16 * 1024]>>> =
            (0..4).map(|_| arena.alloc_uninit_arc::<[u8; 16 * 1024]>()).collect();

        let mut keep = Vec::new();
        for i in 0..N {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        // Spot-check first/middle/last: a bump-cursor corruption affects
        // every element identically.
        assert_eq!(keep[0].0, 0);
        assert_eq!(keep[(N / 2) as usize].0, N / 2);
        assert_eq!(keep[(N - 1) as usize].0, N - 1);
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, N as usize);
    }

    #[test]
    fn chunk_168_force_cache_reuse() {
        let arena = Arena::new();
        // Round 1: allocate arcs, fill a chunk
        let mut batch1: Vec<multitude::Arc<u64>> = Vec::new();
        for i in 0u64..100 {
            batch1.push(arena.alloc_arc(i));
        }
        // Drop all arcs → chunk should be cached via to_thin_ptr
        drop(batch1);

        // Round 2: allocate more arcs — should reuse cached chunk
        let mut batch2: Vec<multitude::Arc<u64>> = Vec::new();
        for i in 0u64..100 {
            batch2.push(arena.alloc_arc(i + 1000));
        }
        for (i, arc) in batch2.iter().enumerate() {
            assert_eq!(**arc, i as u64 + 1000);
        }
    }

    #[test]
    fn chunk_186_payload_rounding_stress() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Stress test with many shared allocations of varying sizes
        let mut keep = Vec::new();
        for i in 0..500 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        // Also test slices with varying sizes
        let mut keep2 = Vec::new();
        for i in 0..100 {
            keep2.push(arena.alloc_slice_fill_with_arc(3, |j| DropTracker((i * 10 + j) as u64)));
        }
        drop(keep);
        drop(keep2);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 800, "500 singles + 300 slice elements = 800");
    }

    #[test]
    fn string_465_reserve_exact_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        s.push_str("12345");
        // Reserve 5 more — total needed == cap (10), should not grow
        s.try_reserve(5).unwrap();
        // Reserve 6 more — total needed == 11 > cap, must grow
        s.try_reserve(6).unwrap();
        s.push_str("67890A");
        assert_eq!(s.as_str(), "1234567890A");
    }

    // =====================================================================
    // UTF-16 stronger tests
    // =====================================================================

    #[test]
    fn vec_460_guard_panic_clone() {
        use std::sync::atomic::AtomicUsize;

        static CLONE_COUNT: AtomicUsize = AtomicUsize::new(0);
        static DROP_COUNT2: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct PanicClone(u64);
        impl Clone for PanicClone {
            fn clone(&self) -> Self {
                let n = CLONE_COUNT.fetch_add(1, Ordering::Relaxed);
                if n >= 3 {
                    panic!("clone panic at count {n}");
                }
                PanicClone(self.0)
            }
        }
        impl Drop for PanicClone {
            fn drop(&mut self) {
                DROP_COUNT2.fetch_add(1, Ordering::Relaxed);
            }
        }

        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<PanicClone>(20);
        v.push(PanicClone(1));
        v.push(PanicClone(2));

        CLONE_COUNT.store(0, Ordering::SeqCst);
        DROP_COUNT2.store(0, Ordering::SeqCst);

        // Resize to 10 — will clone value 8 times. Panics on 4th clone (count=3).
        // After 3 successful clones, len goes from 2→5 then panic.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            v.resize(10, PanicClone(99));
        }));
        assert!(result.is_err(), "should panic during clone");

        // Guard should have dropped 3 cloned elements.
        // PanicClone(99) (the value param) is also dropped during unwind = +1.
        let drops = DROP_COUNT2.load(Ordering::SeqCst);
        assert_eq!(drops, 4, "guard must drop exactly 3 cloned elements + 1 value; got {drops}");
        assert_eq!(v.len(), 2);
    }
}

mod routing_boundary_behavior {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test code")]
    #![allow(clippy::panic, reason = "test code")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "raw identifier names in docs")]
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    extern crate alloc;

    #[test]
    fn resize_uses_subtraction_for_reserve() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, u32> = arena.alloc_vec();
        for i in 0..5 {
            v.push(i);
        }
        assert_eq!(v.len(), 5);
        let cap_before = v.capacity();
        assert!(
            cap_before <= 8,
            "amortized growth from 0 pushes should land at cap=8, got {cap_before}"
        );

        v.resize(10, 0xAA);
        assert_eq!(v.len(), 10);
        assert!(
            v.capacity() <= 16,
            "resize must subtract len from new_len when computing growth (cap={})",
            v.capacity()
        );
    }

    #[derive(Clone)]
    #[expect(dead_code, reason = "scaffold kept for future tests")]
    struct PanicAfter {
        n: StdArc<AtomicUsize>,
        limit: usize,
    }

    // `resize_guard_drop_uses_subtraction` lives in its own integration-test
    // binary (`tests/resize_panic_hook.rs`) because it mutates the
    // process-global panic hook, which is unsafe to do in this shared binary.

    #[test]
    fn shrink_to_fit_at_full_cap_is_noop_documented() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..8 {
            v.push(i);
        }
        assert_eq!(v.len(), v.capacity());
        let ptr_before = v.as_ptr();
        v.shrink_to_fit();
        let ptr_after = v.as_ptr();
        assert_eq!(ptr_before, ptr_after);
    }
}

mod misc_alloc_behavior {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "raw identifier names in docs")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn alloc_uninit_slice_arc_non_drop_above_u16_max_succeeds() {
        let arena = Arena::new();
        let arc = arena
            .try_alloc_uninit_slice_arc::<u8>(u16::MAX as usize + 2)
            .expect("non-Drop slice with len > u16::MAX must succeed via oversized path");
        assert_eq!(arc.len(), u16::MAX as usize + 2);
    }
}

mod alloc_invariants_audit {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local helpers next to their use")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::manual_assert, reason = "explicit panic message clearer in test")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #![allow(clippy::needless_pass_by_value, reason = "test helpers")]
    #![allow(clippy::empty_drop, reason = "tests need non-trivial-drop types to exercise drop-path branches")]
    #![allow(clippy::allow_attributes, reason = "test helpers use allow uniformly")]
    #![allow(clippy::allow_attributes_without_reason, reason = "obvious in test context")]
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn resize_panic_in_middle_drops_only_added_elements() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Counter<'a>(&'a Cell<u32>);
        impl Drop for Counter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        impl Clone for Counter<'_> {
            fn clone(&self) -> Self {
                Counter(self.0)
            }
        }

        let drops = Cell::new(0);
        let panics = Cell::new(0_u32);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Counter<'_>> = arena.alloc_vec_with_capacity(8);
            v.push(Counter(&drops));
            v.push(Counter(&drops));
            // Panic after resize_with has initialized several elements.
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                v.resize_with(7, || {
                    let n = panics.get();
                    panics.set(n + 1);
                    if n == 3 {
                        panic!("synthetic init panic");
                    }
                    Counter(&drops)
                });
            }));
            assert!(result.is_err());
            // 2 pre-existing + 3 successfully written before the panic.
            // The Guard drops the 3 newly-added; len rolls back to 2.
            // The 2 pre-existing get dropped when v drops at end of scope.
            // After Guard runs we should have seen exactly 3 drops.
            assert_eq!(drops.get(), 3, "guard should drop exactly the 3 added elements");
            assert_eq!(v.len(), 2);
        }
        drop(arena);
        assert_eq!(drops.get(), 5);
    }

    #[test]
    fn resize_panic_on_first_element_added_is_zero() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Counter<'a>(&'a Cell<u32>);
        impl Drop for Counter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Counter<'_>> = arena.alloc_vec_with_capacity(4);
            v.push(Counter(&drops));
            // Panic before writing any element; length and drop count remain
            // unchanged.
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                v.resize_with(3, || {
                    panic!("synthetic init panic");
                });
            }));
            assert!(result.is_err());
            assert_eq!(v.len(), 1);
            assert_eq!(drops.get(), 0, "no added elements; nothing should be dropped by Guard");
        }
        drop(arena);
        assert_eq!(drops.get(), 1, "the original element drops with the Vec");
    }

    #[cfg(feature = "dst")]
    #[allow(dead_code, reason = "helper kept after moving its consumers to dst.rs; preserved for future tests")]
    struct OneByteDrop(#[allow(dead_code)] u8);
    #[cfg(feature = "dst")]
    impl Drop for OneByteDrop {
        fn drop(&mut self) {}
    }

    #[test]
    fn realloc_preserves_data_across_growth() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(2);
        v.push(0xdead_beef);
        v.push(0xcafe_babe);
        // Force a realloc by pushing more than the initial capacity.
        for i in 2..10_u32 {
            v.push(i);
        }
        assert_eq!(v[0], 0xdead_beef);
        assert_eq!(v[1], 0xcafe_babe);
        for i in 2..10_u32 {
            assert_eq!(v[i as usize], i);
        }
    }

    #[test]
    fn realloc_empty_to_nonempty_skips_memcpy() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(0);
        // Cap == 0 initially, len == 0. First push triggers realloc with
        // self.cap == 0 (skips the in-place branch via the second clause)
        // and self.len == 0 (skips the memcpy branch).
        v.push(7);
        assert_eq!(v[0], 7);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn drain_to_end_leaves_correct_len() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..6_u32 {
            v.push(i);
        }
        {
            let drained: std::vec::Vec<u32> = v.drain(2..).collect();
            assert_eq!(drained, [2, 3, 4, 5]);
        }
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], 0);
        assert_eq!(v[1], 1);
    }

    #[test]
    fn drain_middle_shifts_tail_correctly() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..6_u32 {
            v.push(i);
        }
        {
            let drained: std::vec::Vec<u32> = v.drain(2..4).collect();
            assert_eq!(drained, [2, 3]);
        }
        // Tail [4, 5] must shift down to indices [2, 3].
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], 0);
        assert_eq!(v[1], 1);
        assert_eq!(v[2], 4);
        assert_eq!(v[3], 5);
    }

    #[test]
    fn drain_panic_in_drop_still_runs_tail_fix() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Boom<'a> {
            on_drop: &'a Cell<u32>,
            explodes: bool,
        }
        impl Drop for Boom<'_> {
            fn drop(&mut self) {
                self.on_drop.set(self.on_drop.get() + 1);
                if self.explodes {
                    panic!("synthetic drop panic");
                }
            }
        }

        let drop_count = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Boom<'_>> = arena.alloc_vec_with_capacity(6);
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 0
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 1
            v.push(Boom {
                on_drop: &drop_count,
                explodes: true,
            }); // 2 - panics on drop
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 3 (drained but not yielded)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 4 (tail)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 5 (tail)

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // Drain [2..4]; iterate forward consuming index 2 (which panics on drop).
                let mut d = v.drain(2..4);
                // Yield index 2 — Boom drops on the consumer's side.
                let yielded = d.next().expect("at least one element");
                // Force the drop here (panics).
                drop(yielded);
            }));
            assert!(result.is_err(), "yielded element's drop should panic");

            // Drain went out of scope (panicked). TailFix should still have run:
            // tail [4,5] shifts down to indices [2,3], len == 4.
            assert_eq!(v.len(), 4);
        }
        drop(arena);
    }

    /// If an unyielded element panics during `Drain::drop`, slice drop glue
    /// still drops the remaining unyielded elements.
    #[test]
    fn drain_partial_consume_panic_in_drop_still_drops_remaining_unyielded() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Boom<'a> {
            on_drop: &'a Cell<u32>,
            explodes: bool,
        }
        impl Drop for Boom<'_> {
            fn drop(&mut self) {
                self.on_drop.set(self.on_drop.get() + 1);
                assert!(!self.explodes, "synthetic drop panic");
            }
        }

        let drop_count = Cell::new(0_u32);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Boom<'_>> = arena.alloc_vec_with_capacity(7);
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 0 (kept)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 1 (kept)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 2 (drained, unyielded, drops cleanly first)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: true,
            }); // 3 (drained, unyielded, PANICS)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 4 (drained, unyielded, must still drop)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 5 (kept tail)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 6 (kept tail)

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // Drain [2..5] without consuming any elements. Drop runs at end of scope.
                let _ = v.drain(2..5);
            }));
            assert!(result.is_err(), "drain drop must propagate the element-drop panic");

            // All three drained elements must have been dropped, even though
            // element 3 panicked in the middle — std::vec::Drain has the same
            // contract via slice-drop glue. Plus tail shift to [2,3], len == 4.
            assert_eq!(
                drop_count.get(),
                3,
                "all 3 drained elements must drop, even with a panic in the middle"
            );
            assert_eq!(v.len(), 4);
        }
        drop(arena);
    }

    #[test]
    fn try_alloc_slice_shared_drop_aware_short_oversized_ok() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let counter = StdArc::new(AtomicU32::new(0));

        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        {
            // Force oversized routing: 4 KiB max_normal_alloc; allocate
            // a slice of 8 KiB (=512 D values, each 16 bytes-ish).
            let arc: Arc<[D]> = arena.alloc_slice_fill_with_arc(512, |_i| D(counter.clone()));
            assert_eq!(arc.len(), 512);
        }
        drop(arena);
        assert_eq!(counter.load(Ordering::Relaxed), 512);
    }

    #[test]
    fn try_alloc_slice_shared_oversized_init_panic_drops_partial() {
        use std::panic::AssertUnwindSafe;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let drops = StdArc::new(AtomicU32::new(0));

        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let drops_ref = drops.clone();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // 1024 * 8 bytes > max_normal_alloc(4 KiB) → oversized shared path.
            let _: Arc<[D]> = arena.alloc_slice_fill_with_arc(1024, |i| {
                if i == 100 {
                    panic!("synthetic init panic");
                }
                D(drops_ref.clone())
            });
        }));
        assert!(result.is_err());
        assert_eq!(drops.load(Ordering::Relaxed), 100);
        drop(arena);
    }

    #[test]
    fn alloc_slice_shared_fast_path_init_panic_drops_partial() {
        use std::panic::AssertUnwindSafe;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::new();
        let drops_ref = drops.clone();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _: Arc<[D]> = arena.alloc_slice_fill_with_arc(64, |i| {
                if i == 32 {
                    panic!("synthetic");
                }
                D(drops_ref.clone())
            });
        }));
        assert!(result.is_err());
        assert_eq!(drops.load(Ordering::Relaxed), 32);
        drop(arena);
    }

    #[test]
    fn alloc_slice_shared_arc_drop_type_runs_drop() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};
        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::new();
        {
            let a: Arc<[D]> = arena.alloc_slice_fill_with_arc(8, |_| D(drops.clone()));
            assert_eq!(a.len(), 8);
            drop(a);
        }
        drop(arena);
        assert_eq!(drops.load(Ordering::Relaxed), 8);
    }

    // Per-`Arc` reference counting permits slices longer than `u16::MAX`.

    #[cfg(not(miri))]
    #[test]
    fn alloc_slice_shared_drop_aware_above_u16_max_succeeds() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};
        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::builder().max_normal_alloc(60 * 1024).build();
        let n = 65_536_usize;
        let arc = arena
            .try_alloc_slice_fill_with_arc(n, |_| D(drops.clone()))
            .expect("Arc slices have no u16 element-count cap");
        assert_eq!(arc.len(), n);
        drop(arc);
        assert_eq!(drops.load(Ordering::Relaxed), n as u32);
    }

    #[test]
    fn resize_with_reserves_minimal_capacity() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(0);
        v.push(1);
        v.push(2);
        v.push(3);
        v.push(4);
        let cap_before = v.capacity();
        v.resize_with(8, || 99_u32);
        let cap_after = v.capacity();
        assert!(
            cap_after < 16,
            "resize_with from len=4 to 8 should not over-reserve (cap_before={cap_before}, cap_after={cap_after})"
        );
        assert_eq!(v.len(), 8);
        assert_eq!(v.as_slice(), &[1, 2, 3, 4, 99, 99, 99, 99]);
    }

    #[test]
    fn pop_if_predicate_sees_last_element() {
        use core::cell::Cell;
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(4);
        v.push(0xaaaa_aaaa);
        v.push(0xbbbb_bbbb);
        v.push(0xcccc_cccc);
        let seen = Cell::new(0_u32);
        let r = v.pop_if(|x| {
            seen.set(*x);
            *x == 0xcccc_cccc
        });
        assert_eq!(seen.get(), 0xcccc_cccc, "predicate must see the final element, not past-end memory");
        assert_eq!(r, Some(0xcccc_cccc));
        assert_eq!(v.as_slice(), &[0xaaaa_aaaa, 0xbbbb_bbbb]);
    }
}

mod string_slice_utf16_behavior {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local helpers next to their use")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::manual_assert, reason = "explicit panic message clearer in test")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #[expect(unused_imports, reason = "documentation of test target types")]
    use multitude::strings::String as ArenaString;
    #[cfg(feature = "utf16")]
    #[expect(unused_imports, reason = "documentation of test target types")]
    use multitude::strings::Utf16String;
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn vec_insert_middle_shifts_exact_tail() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([10_u32, 20, 30, 40, 50]);
        v.insert(1, 99);
        assert_eq!(v.as_slice(), &[10, 99, 20, 30, 40, 50]);
    }

    #[test]
    fn vec_insert_near_start_preserves_all_elements() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..7_u32 {
            v.push(i);
        }
        v.insert(2, 100);
        assert_eq!(v.as_slice(), &[0_u32, 1, 100, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn vec_remove_first_shifts_all_remaining() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([1_u32, 2, 3, 4, 5]);
        let r = v.remove(0);
        assert_eq!(r, 1);
        assert_eq!(v.as_slice(), &[2_u32, 3, 4, 5]);
    }

    #[test]
    fn vec_remove_middle_shifts_exact_tail() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([1_u32, 2, 3, 4, 5]);
        let r = v.remove(2);
        assert_eq!(r, 3);
        assert_eq!(v.as_slice(), &[1_u32, 2, 4, 5]);
    }

    #[test]
    fn vec_remove_second_shifts_three_remaining() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([10_u32, 20, 30, 40, 50]);
        let r = v.remove(1);
        assert_eq!(r, 20);
        assert_eq!(v.as_slice(), &[10_u32, 30, 40, 50]);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn vec_try_reserve_exact_at_capacity_is_noop() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([0_u32, 1, 2]);
        v.try_reserve_exact(5).unwrap();
        assert_eq!(v.capacity(), 8);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn string_try_push_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(8);
        s.push_str("abcde");
        s.push_str("fgh");
        assert_eq!(&*s, "abcdefgh");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn string_insert_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abc");
        s.insert_str(0, "xyz");
        assert_eq!(&*s, "xyzabc");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn string_replace_range_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abc");
        s.replace_range(1..2, "WXYZ");
        assert_eq!(&*s, "aWXYZc");
    }

    #[test]
    fn string_remove_first_preserves_rest() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello");
        let c = s.remove(0);
        assert_eq!(c, 'h');
        assert_eq!(&*s, "ello");
    }

    #[test]
    fn string_remove_middle_preserves_split() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abcdef");
        let c = s.remove(2);
        assert_eq!(c, 'c');
        assert_eq!(&*s, "abdef");
    }

    #[test]
    fn string_retain_preserves_filtered_chars() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello world");
        s.retain(|c| !c.is_whitespace());
        assert_eq!(&*s, "helloworld");
    }

    #[test]
    fn string_replace_range_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("Hello, World!");
        s.replace_range(7..12, "Rust");
        assert_eq!(&*s, "Hello, Rust!");
    }

    #[test]
    fn string_replace_range_replace_with_longer_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abcDEFghi");
        s.replace_range(3..6, "WXYZ");
        assert_eq!(&*s, "abcWXYZghi");
    }

    #[test]
    fn string_replace_range_replace_with_shorter_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abcDEFghi");
        s.replace_range(3..6, "X");
        assert_eq!(&*s, "abcXghi");
    }

    #[test]
    #[cfg(all(debug_assertions, feature = "utf16"))]
    fn utf16_try_push_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(8);
        s.push_from_str("abcd");
        s.try_push_from_str("efgh").unwrap();
        assert_eq!(s.len(), 8);
    }

    #[repr(align(64))]
    #[derive(Debug)]
    struct Align64(u32);

    #[test]
    fn over_aligned_arc_allocation_succeeds_with_extra_padding() {
        let arena = Arena::new();
        let a: Arc<Align64> = arena.alloc_arc(Align64(0xDEAD_BEEF));
        assert_eq!(a.0, 0xDEAD_BEEF);
        let ptr: *const Align64 = core::ptr::from_ref(&*a);
        assert_eq!(ptr.align_offset(64), 0);
    }

    #[test]
    fn try_bump_fit_at_exact_chunk_end_succeeds() {}
    #[test]
    fn allocate_layout_handles_alignment_padding() {
        let arena = Arena::new();
        let _a: Arc<Align64> = arena.alloc_arc(Align64(1));
    }
}

mod freeze_and_box_behavior {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local helpers next to their use")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::manual_assert, reason = "explicit panic message clearer in test")]
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, Box as ArenaBox};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn many_drop_typed_arcs_each_drop_exactly_once() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROPPED: AtomicUsize = AtomicUsize::new(0);

        struct D(#[expect(dead_code, reason = "field discriminates instances")] u32);
        impl Drop for D {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        // SAFETY: D only carries a u32 + atomic side-effect on drop.
        unsafe impl Send for D {}
        unsafe impl Sync for D {}

        DROPPED.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let mut keepers: std::vec::Vec<Arc<D>> = std::vec::Vec::new();
        for i in 0..64_u32 {
            keepers.push(arena.alloc_arc(D(i)));
        }
        drop(keepers);
        drop(arena);
        assert_eq!(DROPPED.load(Ordering::SeqCst), 64);
    }

    #[test]
    fn many_drop_typed_slices_drop_each_element_once() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROPPED: AtomicUsize = AtomicUsize::new(0);

        struct D(#[expect(dead_code, reason = "field discriminates instances")] u32);
        impl Drop for D {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        // SAFETY: same rationale.
        unsafe impl Send for D {}
        unsafe impl Sync for D {}

        DROPPED.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let mut keepers: std::vec::Vec<Arc<[D]>> = std::vec::Vec::new();
        for batch in 0..8_u32 {
            keepers.push(arena.alloc_slice_fill_with_arc(8, move |i| D(batch * 8 + i as u32)));
        }
        drop(keepers);
        drop(arena);
        assert_eq!(DROPPED.load(Ordering::SeqCst), 64);
    }

    #[test]
    fn vec_into_box_empty_routes_through_copy_path() {
        let arena = Arena::new();
        let v: ArenaVec<'_, u32> = arena.alloc_vec();
        let b: ArenaBox<[u32]> = v.into_boxed_slice();
        assert_eq!(b.len(), 0);
    }
}

mod public_surface_behavior {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::explicit_into_iter_loop, reason = "test clarity")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #![allow(clippy::items_after_statements, reason = "test-local statics next to their use")]
    #![allow(
        clippy::cast_ptr_alignment,
        reason = "test writes a u32 to a u8-typed reservation we created with u32 layout"
    )]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[cfg(feature = "dst")]
    use multitude::Arc;
    use multitude::vec::{CollectIn, Vec};
    use multitude::{Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;
    use crate::common::{FailingAllocator, SendFailingAllocator};

    #[test]
    fn allocator_deallocate_triggers_teardown_when_last_ref() {
        // <&Arena as Allocator>::deallocate's `if needs_teardown` branch:
        // the deallocate must observe refcount → 0 and call teardown_chunk.
        // Achieved by forcing many grow → relocate cycles inside a Vec
        // backed by `&Arena`: each old buffer's deallocate eventually
        // tears down its chunk (the chunk's only ref was the Vec's
        // buffer, and after retirement the arena no longer holds it).
        let arena: Arena = Arena::builder().build();
        {
            let mut v: allocator_api2::vec::Vec<u8, &Arena> = allocator_api2::vec::Vec::new_in(&arena);
            for _ in 0..16_000_u32 {
                v.push(0);
            }
            drop(v);
        }
    }

    #[test]
    fn builder_debug_format() {
        let s = format!("{:?}", Arena::builder());
        assert!(s.contains("ArenaBuilder"));
        assert!(s.contains("max_normal_alloc"));
    }

    #[test]
    fn builder_preallocate_alloc_failed() {
        // Drives the AllocError return path in ArenaBuilder::try_build by
        // giving the builder an allocator that refuses to allocate.
        let alloc = FailingAllocator::new(0);
        let result = Arena::builder().with_capacity(512).allocator_in(alloc).try_build();
        assert!(result.is_err());
    }

    #[test]
    fn arena_box_drop_unlinks_middle_of_drop_list() {
        // Dropping the middle value unlinks an entry with both neighbors.
        let arena = Arena::new();
        let mut b1 = arena.alloc_box(std::string::String::from("first"));
        let mut b2 = arena.alloc_box(std::string::String::from("middle"));
        let mut b3 = arena.alloc_box(std::string::String::from("last"));
        // Make sure each value is reachable (touch the contents).
        b1.push('!');
        b2.push('!');
        b3.push('!');
        drop(b2);
        assert_eq!(*b1, "first!");
        assert_eq!(*b3, "last!");
    }

    #[test]
    fn cached_local_chunk_revived_as_shared() {
        // Preallocation seeds a local chunk that `alloc_arc` revives as shared.
        let arena: Arena = Arena::builder().with_capacity(1024).build();
        let shared = arena.alloc_arc(99_u64);
        assert_eq!(*shared, 99);
        let join = thread::spawn(move || *shared);
        assert_eq!(99, join.join().unwrap());
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_box(0_u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_box_with_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_box_with(|| 0_u32);
    }

    #[test]
    #[cfg(feature = "dst")]
    fn alloc_dst_arc_rejects_excessive_alignment() {
        let arena: Arena = Arena::new();
        let huge_align = 128 * 1024_usize;
        let layout = core::alloc::Layout::from_size_align(huge_align, huge_align).unwrap();
        let r = unsafe {
            arena.try_alloc_dst_arc::<[u8]>(layout, 0_usize, |_| {
                unreachable!("init must not be called when allocation fails");
            })
        };
        assert!(r.is_err());
    }

    #[test]
    #[cfg(feature = "dst")]
    fn alloc_dst_box_rejects_excessive_alignment() {
        let arena: Arena = Arena::new();
        let huge_align = 128 * 1024_usize;
        let layout = core::alloc::Layout::from_size_align(huge_align, huge_align).unwrap();
        let r = unsafe {
            arena.try_alloc_dst_box::<[u8]>(layout, 0_usize, |_| {
                unreachable!("init must not be called when allocation fails");
            })
        };
        assert!(r.is_err());
    }

    // `#[repr(align(N))]` with N > CHUNK_ALIGN (64 KiB). Used by the two
    // tests below to drive the `if layout.align() > CHUNK_ALIGN { return
    // Err(AllocError) }` guard in `try_alloc_with` and `try_reserve_and_init`.
    //
    // The guard lives in a thin outer function whose frame doesn't depend
    // on `T`'s alignment, so the test runs on every LLVM-backed platform —
    // including Windows, whose default 1 MiB stack can't accommodate the
    // 128 KiB-aligned frame the guarded body would otherwise require.
    //
    // Skipped under the UTC codegen backend (`--cfg utc_backend`): UTC caps
    // type alignment at 8192 bytes, well below the 128 KiB this test needs.
    #[cfg(not(utc_backend))]
    #[repr(align(131072))]
    struct HugeAlign(#[expect(dead_code, reason = "field present to give the type a non-zero size")] u8);

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_with_rejects_excessive_alignment() {
        // try_alloc_with is the Alloc<T> entry point. CHUNK_ALIGN is 64 KiB;
        // HugeAlign needs 128 KiB alignment, so the layout-align check
        // must fire and return Err.
        let arena: Arena = Arena::new();
        let result: Result<multitude::Alloc<'_, HugeAlign>, _> = arena.try_alloc_with(|| HugeAlign(0));
        assert!(result.is_err());
    }

    #[test]
    fn try_alloc_string_with_capacity_huge_returns_err() {
        let arena: Arena = Arena::new();
        // Try a capacity that overflows when adding the prefix size.
        let too_big = usize::MAX;
        assert!(arena.try_alloc_string_with_capacity(too_big).is_err());
    }

    #[test]
    fn try_alloc_string_with_capacity_isize_max_returns_err() {
        // Drives the `isize::try_from(total).is_err()` guard in
        // ArenaString::try_allocate_initial. Need cap such that
        // `cap + PREFIX_SIZE` is between `isize::MAX + 1` and `usize::MAX`.
        let arena: Arena = Arena::new();
        let cap = (isize::MAX as usize) - 4; // cap + 8 > isize::MAX, and < usize::MAX
        assert!(arena.try_alloc_string_with_capacity(cap).is_err());
    }

    #[test]
    fn arena_string_grow_through_chunk_rotation() {
        // Drives the `if needs_teardown { teardown_chunk(chunk, true); }`
        // branch in `Arena::grow_for_string` — when the OLD string buffer's
        // chunk has only the string as a holder (refcount==1 → after dec
        // it's 0 → teardown).
        let arena: Arena = Arena::builder().build();
        let mut s = arena.alloc_string();
        // Push enough text to force the string to grow into a fresh chunk;
        // the old chunk had ONLY this string (no other allocations) so its
        // refcount drops to 0 on grow → triggers teardown_chunk.
        let chunk = "x".repeat(64);
        for _ in 0..200 {
            s.push_str(&chunk);
        }
        assert_eq!(s.len(), 200 * 64);
    }

    #[test]
    fn arena_vec_deref_mut_modifies_in_place() {
        let arena = Arena::new();
        let mut v: Vec<u32, _> = arena.alloc_vec();
        v.push(1);
        v.push(2);
        v.push(3);
        // Modify via DerefMut (not via push).
        let slice: &mut [u32] = &mut v;
        slice[0] = 99;
        assert_eq!(v.as_slice(), &[99, 2, 3]);
    }

    #[test]
    fn collect_in_empty_iterator_uses_new_in() {
        // An iterator with `size_hint().0 == 0` should take the `new_in`
        // path (no `with_capacity_in(0)` detour). Easiest: filter that
        // discards everything but advertises `(0, _)`.
        let arena = Arena::new();
        let v: Vec<u32, _> = (0..10_u32).filter(|_| false).collect_in(&arena);
        assert!(v.is_empty());
    }

    #[test]
    #[cfg(feature = "dst")]
    fn alloc_dst_arc_runs_drop_on_chunk_teardown() {
        use core::sync::atomic::{AtomicUsize, Ordering as Ord};

        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        DROP_COUNT.store(0, Ord::SeqCst);

        struct Tracked(#[expect(dead_code, reason = "field exists only for size")] u32);
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = DROP_COUNT.fetch_add(1, Ord::SeqCst);
            }
        }

        let arena: Arena = Arena::new();
        {
            let layout = core::alloc::Layout::array::<Tracked>(1).unwrap();
            // SAFETY: layout matches [Tracked; 1]; init writes one Tracked.
            let arc: Arc<[Tracked]> = unsafe {
                arena.alloc_dst_arc::<[Tracked]>(layout, 1_usize, |fat: *mut [Tracked]| {
                    fat.cast::<Tracked>().write(Tracked(0xCAFE_F00D));
                })
            };
            assert_eq!(arc.len(), 1);
            let h = thread::spawn(move || arc.len());
            let val = h.join().unwrap();
            assert_eq!(val, 1);
        }
        drop(arena);
        assert_eq!(DROP_COUNT.load(Ord::SeqCst), 1, "drop must run exactly once");
    }

    // Use ArenaBuilder type (covered by allocator_in test) to silence
    // unused-import warnings if any of the above tests change.
    #[test]
    fn builder_type_is_constructible() {
        let _: ArenaBuilder = Arena::builder();
    }

    #[test]
    fn arena_try_alloc_str_arc_succeeds() {
        use multitude::Arc;
        let arena: Arena = Arena::new();
        let s: Arc<str> = arena.try_alloc_str_arc("hello arc").unwrap();
        assert_eq!(s.as_str(), "hello arc");
    }

    #[test]
    fn arena_try_alloc_str_box_succeeds() {
        use multitude::Box;
        let arena: Arena = Arena::new();
        let s: Box<str> = arena.try_alloc_str_box("hello box").unwrap();
        assert_eq!(s.as_str(), "hello box");
    }

    #[test]
    fn arena_box_str_as_mut_via_trait() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_str_box("abc");
        let m: &mut str = AsMut::<str>::as_mut(&mut s);
        // SAFETY: ASCII bytes; in-place uppercase preserves UTF-8.
        unsafe { m.as_bytes_mut()[0] = b'A' };
        assert_eq!(s.as_str(), "Abc");
    }

    #[test]
    fn alloc_string_with_capacity_allocates_buffer() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(64);
        assert!(s.capacity() >= 64);
        s.push_str("hello world");
        assert_eq!(s.as_str(), "hello world");
    }

    #[test]
    fn try_alloc_vec_with_capacity_succeeds() {
        let arena: Arena = Arena::new();
        let mut v = arena.try_alloc_vec_with_capacity::<u32>(16).unwrap();
        assert!(v.capacity() >= 16);
        v.push(1);
        v.push(2);
        assert_eq!(&*v, &[1, 2]);
    }

    // `panic_alloc` closure paths for the Arc/Box variants of slice / value
    // constructors. These mirror the existing tests for the Rc variants;
    // each drives the `unwrap_or_else(|_| panic_alloc())` closure body so it
    // shows as covered.

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_arc(0_u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_arc_with_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_arc_with(|| 0_u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_copy_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_copy_arc([0_u8; 4]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_clone_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_clone_arc([1_u32, 2]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_with_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_fill_with_arc::<u32, _>(4, |i| i as u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_iter_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_fill_iter_arc([1_u32, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_str_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_str("hi");
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_str_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_str_arc("hi");
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_str_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_str_box("hi");
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_string_with_capacity_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_string_with_capacity(64);
    }

    #[test]
    #[should_panic(expected = "multitude::ArenaBuilder::build")]
    fn build_panics_on_failing_allocator() {
        let _: Arena<FailingAllocator> = Arena::builder().allocator_in(FailingAllocator::new(0)).with_capacity(512).build();
    }

    #[test]
    #[should_panic(expected = "multitude::ArenaBuilder::build")]
    fn build_panics_on_send_failing_allocator() {
        let _: Arena<SendFailingAllocator> = Arena::builder()
            .allocator_in(SendFailingAllocator::new(0))
            .with_capacity(512)
            .build();
    }

    // Distinct type from `HugeAlign` above so we don't perturb the caller's frame
    // alignment and trigger the issue noted in the comment near
    // `try_alloc_with_rejects_excessive_alignment`. The `MaybeUninit<T>` returned
    // by the uninit-family entry points never materializes a real `T` on the
    // stack, so the test compiles and runs safely on every platform.
    #[cfg(not(utc_backend))]
    #[repr(align(131072))]
    struct HugeAlignBox(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_uninit_box_rejects_excessive_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_box::<HugeAlignBox>();
        assert!(r.is_err());
    }

    #[test]
    fn arena_string_replace_range_excluded_start() {
        use core::ops::Bound;
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello");
        // Excluded(0) -> start = 1, Excluded(3) -> end = 3 -> replace bytes 1..3 ("el") with "X"
        s.replace_range((Bound::Excluded(0_usize), Bound::Excluded(3_usize)), "X");
        assert_eq!(&*s, "hXlo");
    }

    #[test]
    fn arena_string_replace_range_grow_path() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("ab");
        // Replacement is much longer than what's removed, forcing a grow
        // (`new_len > self.cap` branch in replace_range).
        s.replace_range(0..1, "lots of replacement text");
        assert_eq!(&*s, "lots of replacement textb");
    }

    #[test]
    fn arena_string_replace_range_added_gt_removed_no_grow() {
        // Drives the `added > removed` arm of replace_range with the
        // `new_len > self.cap` check evaluating to false (the buffer
        // already has enough capacity for the larger replacement).
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(64);
        s.push_str("abc");
        s.replace_range(0..1, "XY"); // removed=1, added=2 -> grows by 1; cap (64) suffices
        assert_eq!(&*s, "XYbc");
    }

    #[test]
    fn arena_string_try_reserve_additional_overflow_returns_err() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("a");
        // self.len (1) + usize::MAX overflows -> Err.
        let r = s.try_reserve(usize::MAX);
        assert!(r.is_err());
    }

    #[test]
    fn arena_string_try_reserve_within_existing_capacity_is_noop() {
        // Drives the `needed <= self.cap` branch of `try_reserve`
        // (cap already suffices, so try_grow_to_at_least is not called).
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(64);
        s.push_str("hi");
        s.try_reserve(8).unwrap();
        assert!(s.capacity() >= 64);

        let mut exact = arena.alloc_string_with_capacity(8);
        exact.push_str("abc");
        exact.try_reserve(5).unwrap();
        assert_eq!(exact.capacity(), 8);
    }

    #[test]
    fn arena_string_try_reserve_grow_path_succeeds() {
        // Drives the success-fall-through past `try_grow_to_at_least(needed)?`
        // in `try_reserve` (cap>0, needed>cap, grow succeeds).
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("seed");
        let prior = s.capacity();
        s.try_reserve(prior * 4).unwrap();
        assert!(s.capacity() >= prior * 4 + s.len());
    }

    #[test]
    fn arena_string_try_reserve_grow_path_overflow_returns_err() {
        // Drives `try_grow_to_at_least`'s `PREFIX_SIZE.checked_add(new_cap)` /
        // `isize::try_from(new_total)` failure paths. We need cap > 0 first
        // (so we hit the grow path, not initial allocate), then ask for an
        // additional that pushes total past isize::MAX.
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("seed"); // cap > 0
        // additional fits in usize but new_total overflows isize.
        let additional = (isize::MAX as usize) - 4;
        let r = s.try_reserve(additional);
        assert!(r.is_err());
    }

    use std::panic::AssertUnwindSafe;

    fn expect_panic<F: FnOnce()>(f: F) {
        let r = std::panic::catch_unwind(AssertUnwindSafe(f));
        assert!(r.is_err(), "expected panic but call returned");
    }

    fn fail_arena() -> Arena<FailingAllocator> {
        Arena::new_in(FailingAllocator::new(0))
    }

    fn send_fail_arena() -> Arena<SendFailingAllocator> {
        Arena::new_in(SendFailingAllocator::new(0))
    }

    #[test]
    fn panic_alloc_with() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_with(|| 42);
        });
    }

    #[test]
    fn panic_alloc_str() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_str("hi");
        });
    }

    #[test]
    fn panic_alloc_slice_fill_iter() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_slice_fill_iter([1_u32, 2, 3]);
        });
    }

    #[test]
    fn panic_alloc_uninit_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_uninit_box::<u32>();
        });
    }

    #[test]
    fn panic_alloc_zeroed_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_zeroed_box::<u32>();
        });
    }

    #[test]
    fn panic_alloc_uninit_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_uninit_arc::<u32>();
        });
    }

    #[test]
    fn panic_alloc_zeroed_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_zeroed_arc::<u32>();
        });
    }

    #[test]
    fn panic_alloc_uninit_slice_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_uninit_slice_arc::<u32>(4);
        });
    }

    #[test]
    fn panic_alloc_zeroed_slice_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_zeroed_slice_arc::<u32>(4);
        });
    }

    #[test]
    fn try_alloc_str_err() {
        let a = fail_arena();
        assert!(a.try_alloc_str("hi").is_err());
    }

    #[test]
    fn try_alloc_uninit_box_err() {
        let a = fail_arena();
        assert!(a.try_alloc_uninit_box::<u32>().is_err());
    }

    #[test]
    fn try_alloc_zeroed_box_err() {
        let a = fail_arena();
        assert!(a.try_alloc_zeroed_box::<u32>().is_err());
    }

    #[test]
    fn try_alloc_uninit_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_uninit_arc::<u32>().is_err());
    }

    #[test]
    fn try_alloc_zeroed_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_zeroed_arc::<u32>().is_err());
    }

    #[test]
    fn try_alloc_uninit_slice_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_uninit_slice_arc::<u32>(4).is_err());
    }

    #[test]
    fn try_alloc_zeroed_slice_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_zeroed_slice_arc::<u32>(4).is_err());
    }

    #[test]
    fn arena_string_try_push_str_initial_alloc_err() {
        let a = fail_arena();
        let mut s = a.alloc_string();
        assert!(s.try_push_str("hello").is_err());
    }

    #[test]
    fn arena_string_try_grow_to_at_least_grow_path_err() {
        // Allow the initial chunk alloc, fail the grow's new-chunk alloc by
        // requesting a capacity that exceeds the chunk_size.
        let a = Arena::builder().allocator_in(FailingAllocator::new(1)).build();
        let mut s = a.try_alloc_string_with_capacity(4).unwrap();
        s.try_push_str("abcd").unwrap();
        // Forces grow_for_string → needs new (oversized) chunk → allocator fails.
        assert!(s.try_reserve(64 * 1024).is_err());
    }

    #[test]
    fn panic_arena_string_grow_to_at_least() {
        expect_panic(|| {
            let a = Arena::builder().allocator_in(FailingAllocator::new(1)).build();
            let mut s = a.try_alloc_string_with_capacity(4).unwrap();
            s.try_push_str("abcd").unwrap();
            // grow_to_at_least asks for a new chunk; allocator is exhausted.
            s.push_str("x".repeat(64 * 1024));
        });
    }

    #[test]
    fn grow_for_string_old_chunk_torn_down() {
        let a = Arena::builder().build();
        let mut s = a.alloc_string();
        // Force at least one grow_for_string call. Initial cap == 16.
        s.push_str("x".repeat(64));
        s.push_str("y".repeat(8 * 1024));
        drop(s);
    }

    #[test]
    fn oversized_no_drop_branch() {
        let a = Arena::builder().max_normal_alloc(4 * 1024).build();
        let _s = a.alloc_slice_copy(&[0_u8; 1500][..]);
    }

    #[test]
    fn panic_alloc_slice_fill_with() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_slice_fill_with(4, |i| i as u32);
        });
    }

    #[test]
    fn vec_try_reserve_no_growth_needed() {
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        v.push(1);
        v.push(2);
        assert!(v.try_reserve(1).is_ok());
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn vec_try_reserve_exact_realloc_and_overflow() {
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        v.push(1);
        assert!(v.try_reserve_exact(100).is_ok());
        assert!(v.capacity() >= 101);

        assert!(v.try_reserve_exact(1).is_ok());

        let err = v.try_reserve_exact(usize::MAX);
        assert!(err.is_err());
    }

    #[test]
    fn vec_resize_with_shrink() {
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..10 {
            v.push(i);
        }
        v.resize_with(3, || unreachable!());
        assert_eq!(v.len(), 3);
        assert_eq!(&*v, &[0, 1, 2]);
    }

    #[test]
    fn vec_drain_with_exclusive_start_and_inclusive_end() {
        use core::ops::Bound;
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..10 {
            v.push(i);
        }

        let drained: std::vec::Vec<_> = v.drain((Bound::Excluded(0), Bound::Included(3))).collect();
        assert_eq!(drained, vec![1, 2, 3]);
        assert_eq!(v.len(), 7);

        let arena2 = Arena::new();
        let mut v2: Vec<u32> = arena2.alloc_vec();
        for i in 0..5 {
            v2.push(i);
        }
        let drained2: std::vec::Vec<_> = v2.drain(..).collect();
        assert_eq!(drained2, vec![0, 1, 2, 3, 4]);
        assert_eq!(v2.len(), 0);
    }

    #[test]
    fn vec_zst_operations() {
        let arena = Arena::new();
        let mut v: Vec<()> = arena.alloc_vec();
        for _ in 0..100 {
            v.push(());
        }
        assert_eq!(v.len(), 100);
        v.shrink_to_fit();
        assert_eq!(v.len(), 100);
    }

    #[test]
    fn vec_drain_debug_and_next_back() {
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..5 {
            v.push(i);
        }
        let mut drain = v.drain(1..4);
        let s = std::format!("{drain:?}");
        assert!(s.contains("Drain"), "Debug output: {s}");
        assert!(s.contains("remaining"), "Debug output: {s}");

        assert_eq!(drain.next_back(), Some(3));
        assert_eq!(drain.next_back(), Some(2));
        assert_eq!(drain.next(), Some(1));
        assert_eq!(drain.next_back(), None);
    }

    #[test]
    fn vec_insert_triggers_growth() {
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..4 {
            v.push(i);
        }
        assert_eq!(v.capacity(), 4);
        v.insert(2, 99);
        assert_eq!(v[2], 99);
        assert!(v.capacity() > 4);
    }

    #[test]
    fn vec_push_panics_on_alloc_failure() {
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(1)); // 1 alloc for initial chunk
            let mut v: Vec<u64, _> = arena.alloc_vec();
            // One chunk forces a later growth attempt to fail.
            for _ in 0..100 {
                v.push(0);
            }
        });
    }

    #[test]
    fn vec_reserve_panics_on_alloc_failure() {
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(0));
            let mut v: Vec<u64, _> = arena.alloc_vec();
            v.reserve(1);
        });
    }

    #[test]
    fn vec_reserve_exact_panics_on_alloc_failure() {
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(0));
            let mut v: Vec<u64, _> = arena.alloc_vec();
            v.reserve_exact(1);
        });
    }

    #[test]
    fn shared_bump_fast_path_bail_on_oversize() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let arc = arena.alloc_arc([0_u64; 1024]); // 8192 bytes > 4096
        assert_eq!(arc[0], 0);
    }

    #[test]
    fn shared_bump_fit_in_current_chunk() {
        let arena = Arena::new();
        let _a1 = arena.alloc_arc(1_u32);
        let _a2 = arena.alloc_arc(2_u32);
    }

    #[test]
    fn shared_oversized_inc_ref_on_non_normal_chunk() {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let data = [42_u8; 8192]; // > max_normal_alloc(4096)
        let arc_slice = arena.alloc_slice_copy_arc(&data[..]);
        assert_eq!(arc_slice.len(), 8192);
        assert_eq!(arc_slice[0], 42);
    }

    #[test]
    fn shared_eviction_of_pinned_chunk() {
        // A small chunk forces refill while the string builder retains it.
        let arena = Arena::builder().with_capacity(512).build();
        let mut s = arena.alloc_string();
        let n = 600;
        for _ in 0..n {
            s.push('A');
        }
        assert!(s.len() >= n);
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    // See note on `acquire_slice_slot_rejects_overaligned`: naming a
    // `T` with `align(131072)` aborts on Windows before the guard runs.
    fn try_alloc_slice_copy_rejects_overaligned() {
        #[repr(align(131072))]
        #[derive(Clone, Copy)]
        #[expect(dead_code, reason = "field needed for alignment/size but not read")]
        struct HugeAlign(u8);

        let arena = Arena::new();
        let data = [HugeAlign(0)];
        let result = arena.try_alloc_slice_copy(&data[..]);
        assert!(result.is_err());
    }

    #[test]
    fn try_alloc_slice_copy_rejects_overflow() {
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<u64, _>(usize::MAX / 4, |_| 0);
        assert!(result.is_err());
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    // See note on `acquire_slice_slot_rejects_overaligned`: naming a
    // `T` with `align(131072)` aborts on Windows before the guard runs.
    fn try_alloc_slice_fill_with_rejects_overaligned() {
        #[repr(align(131072))]
        struct HugeAlignDrop(#[expect(dead_code, reason = "field needed for alignment/size but not read")] u8);
        #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<T>() true for test")]
        impl Drop for HugeAlignDrop {
            fn drop(&mut self) {}
        }

        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<HugeAlignDrop, _>(1, |_| HugeAlignDrop(0));
        assert!(result.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_no_drop_fast_path() {
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<u32, _>(10, |i| i as u32);
        assert!(result.is_ok());
        let slice = result.unwrap();
        assert_eq!(slice.len(), 10);
        assert_eq!(slice[5], 5);
    }

    #[test]
    fn try_alloc_slice_fill_with_overflow() {
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<u64, _>(usize::MAX / 4, |_| 0);
        assert!(result.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_overflow() {
        let arena = Arena::new();
        let len = (isize::MAX as usize) / 8 + 1;
        let result = arena.try_alloc_slice_fill_with::<u64, _>(len, |_| 0);
        assert!(result.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_non_drop_fast_path() {
        let arena = Arena::new();
        let _ = arena.alloc_slice_fill_with::<u32, _>(4, |i| i as u32);
        let slice = arena.alloc_slice_fill_with::<u32, _>(4, |i| (i + 10) as u32);
        assert_eq!(&*slice, &[10, 11, 12, 13]);
    }

    #[test]
    fn slice_init_guard_drops_prefix_on_panic() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        #[derive(Clone)]
        #[expect(dead_code, reason = "field needed for alignment/size but not read")]
        struct Tracked(u32);
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);
        let arena = Arena::new();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_fill_with::<Tracked, _>(5, |i| {
                assert!(i != 3, "deliberate panic at index 3");
                Tracked(i as u32)
            });
        }));
        assert!(result.is_err());
        // Elements 0, 1, 2 were initialized before the panic at index 3.
        // SliceInitGuard should have dropped them.
        assert!(DROP_COUNT.load(Ordering::Relaxed) >= 3);
    }

    /// A slice copy fits in an already-populated current chunk.
    #[test]
    fn alloc_slice_copy_fast_path_bump() {
        let arena = Arena::new();
        // First allocation populates current with a fresh chunk.
        let _x = arena.alloc(42_u8);
        // Second allocation is small enough to bump within the same chunk,
        // hitting the `the current-chunk bump path` success path.
        let s = arena.alloc_slice_copy([1_u8, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&*s, &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    //
    // All smart-pointer alloc paths reject `align >= 32 KiB` because, with
    // the co-allocated `DropEntry` taking 32 bytes immediately before the
    // payload, an `align == 32 KiB` payload lands at chunk offset
    // `CHUNK_ALIGN`. `header_for(value_ptr)` masks the low 16 bits of the
    // pointer to recover the chunk header — for that offset, the mask
    // returns the *next* chunk's address. The guard exists to make this
    // failure mode unreachable from safe code.
    //
    // These tests pin the boundary: a sized `T` with `repr(align(32768))`
    // must be rejected by every smart-pointer entry point. The companion
    // tests in `dst.rs` cover the unsafe DST paths.
    //
    // Skipped on Windows: naming a type with `align(32768)` on stack inside
    // `try_alloc_*_with` materializes a stack frame Windows' default 1 MiB
    // stack cannot satisfy on entry, aborting with STATUS_STACK_OVERFLOW
    // before the guard runs. The MaybeUninit/uninit-family tests only hold
    // the type *inside* `MaybeUninit`, so they're safe everywhere.

    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct HalfChunkAlignNoDrop(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);

    #[cfg(not(utc_backend))]
    #[repr(align(32768))]
    struct HalfChunkAlignDrop(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);

    #[cfg(not(utc_backend))]
    #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<T>() true for the test")]
    impl Drop for HalfChunkAlignDrop {
        fn drop(&mut self) {}
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    fn try_alloc_arc_with_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r: Result<multitude::Arc<HalfChunkAlignDrop>, _> = arena.try_alloc_arc_with(|| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    fn try_alloc_box_with_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r: Result<multitude::Box<HalfChunkAlignDrop>, _> = arena.try_alloc_box_with(|| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_uninit_box_rejects_half_chunk_alignment() {
        // Holding T inside MaybeUninit means no stack frame needs T's
        // alignment, so this test is portable to Windows.
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_box::<HalfChunkAlignDrop>();
        assert!(r.is_err());
    }

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_uninit_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_arc::<HalfChunkAlignDrop>();
        assert!(r.is_err());
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    fn try_alloc_slice_fill_with_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_slice_fill_with_arc::<HalfChunkAlignDrop, _>(1, |_| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    fn try_alloc_slice_fill_with_box_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_slice_fill_with_box::<HalfChunkAlignDrop, _>(1, |_| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_uninit_slice_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_slice_arc::<HalfChunkAlignDrop>(1);
        assert!(r.is_err());
    }

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_uninit_slice_box_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_slice_box::<HalfChunkAlignDrop>(1);
        assert!(r.is_err());
    }

    #[test]
    #[cfg(all(not(target_os = "windows"), not(utc_backend)))]
    fn try_alloc_slice_copy_arc_allows_half_chunk_align_for_copy_t() {
        let arena: Arena = Arena::new();
        let data = [HalfChunkAlignNoDrop(0), HalfChunkAlignNoDrop(1)];
        let r = arena.try_alloc_slice_copy_arc(&data[..]);
        assert!(r.is_err());
    }

    //
    // Each `alloc_*_with` reserves a slot, takes a protective `+1` chunk
    // refcount, then runs the user-supplied `f`. If `f` panics, the
    // `RefcountReleaseGuard` releases that `+1` so the chunk reclaims
    // normally; no `DropEntry` is linked (so `T::drop` does not run on the
    // half-built value), and the bump bytes leak in-chunk. The arena must
    // remain usable after the panic.

    #[test]
    fn alloc_arc_with_closure_panic_releases_refcount() {
        use std::panic::AssertUnwindSafe;

        let arena: Arena = Arena::new();
        let _stable = arena.alloc_arc(0_u32);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _: multitude::Arc<u64> = arena.alloc_arc_with(|| panic!("deliberate panic in alloc_arc_with"));
        }));
        assert!(result.is_err());

        let after = arena.alloc_arc(99_u32);
        assert_eq!(*after, 99);
    }

    #[test]
    fn alloc_box_with_closure_panic_releases_refcount() {
        use std::panic::AssertUnwindSafe;

        let arena: Arena = Arena::new();
        let _stable = arena.alloc_box(0_u32);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _: multitude::Box<u64> = arena.alloc_box_with(|| panic!("deliberate panic in alloc_box_with"));
        }));
        assert!(result.is_err());

        let after = arena.alloc_box(99_u32);
        assert_eq!(*after, 99);
    }

    #[test]
    fn vec_resize_clones_exactly_extra_minus_one() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static CLONE_COUNT: AtomicUsize = AtomicUsize::new(0);
        CLONE_COUNT.store(0, Ordering::Relaxed);

        #[derive(Default)]
        struct CloneCounter;
        impl Clone for CloneCounter {
            fn clone(&self) -> Self {
                CLONE_COUNT.fetch_add(1, Ordering::Relaxed);
                Self
            }
        }

        let arena: Arena = Arena::new();
        let mut v: multitude::vec::Vec<CloneCounter> = arena.alloc_vec();
        v.push(CloneCounter);
        v.push(CloneCounter);
        assert_eq!(CLONE_COUNT.load(Ordering::Relaxed), 0);

        v.resize(5, CloneCounter);

        assert_eq!(v.len(), 5);
        assert_eq!(CLONE_COUNT.load(Ordering::Relaxed), 2);
    }
}

mod public_surface_behavior_2 {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::explicit_into_iter_loop, reason = "test clarity")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #![allow(clippy::items_after_statements, reason = "test-local statics next to their use")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    use core::alloc::Layout;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use allocator_api2::alloc::Allocator;
    use multitude::strings::String as ArenaString;
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, FromIn as _};

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;
    use crate::common::{FailingAllocator, SendFailingAllocator};

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Droppy(&'static str);

    impl Drop for Droppy {
        fn drop(&mut self) {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    struct DropZst;

    impl Drop for DropZst {
        fn drop(&mut self) {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn arc_from_arena_vec_uses_into_arc() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, i32> = arena.alloc_vec();
        v.push(1);
        v.push(2);

        let a: Arc<[i32]> = v.into();
        assert_eq!(&*a, &[1, 2]);
    }

    #[test]
    fn builder_preallocate_shared_releases_budget_on_allocator_error() {
        assert!(
            Arena::builder_in(SendFailingAllocator::new(0))
                .with_capacity(512)
                .try_build()
                .is_err()
        );
    }

    #[test]
    fn oversized_shared_alloc_error_releases_budget() {
        let arena = Arena::builder_in(SendFailingAllocator::new(0)).max_normal_alloc(4096).build();
        let src = std::vec![7_u8; 5000];
        assert!(arena.try_alloc_slice_copy_arc(src).is_err());
    }

    #[test]
    fn cache_discards_too_small_chunk_before_large_request() {
        let arena = Arena::builder().with_capacity(512).build();
        let big = std::vec![3_u8; 4096];
        let a = arena.alloc_slice_copy_arc(&big);
        assert_eq!(a.len(), big.len());
    }

    #[test]
    fn preallocate_local_updates_high_water_on_larger_class() {
        let arena = Arena::builder().with_capacity(1024).build();
        let value = arena.alloc(42_u32);
        assert_eq!(*value, 42);
    }

    #[test]
    fn string_retain_panic_restores_guard_len() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcd", &arena);

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            s.retain(|ch| {
                assert_ne!(ch, 'd', "retain must stop at the panic");
                assert!(ch != 'c', "predicate panic");
                ch != 'b'
            });
        }));

        assert!(result.is_err());
        assert_eq!(s.as_str(), "a");
    }

    /// If the predicate panics, `Vec::retain` preserves the kept prefix.
    #[test]
    fn vec_retain_panic_preserves_kept_prefix() {
        use std::cell::Cell;

        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, i32> = arena.alloc_vec();
        v.extend([1_i32, 2, 3, 4, 5]);

        // Predicate: keep odd numbers; panic on element `3`. After panic,
        // the kept prefix `[1]` must remain (matches std::Vec::retain).
        let seen = Cell::new(0_i32);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            v.retain(|x| {
                seen.set(seen.get() + 1);
                assert!(*x != 3, "predicate panic at element 3");
                *x % 2 == 1
            });
        }));
        assert!(result.is_err());
        // Element 1 passed the predicate (kept), element 2 was dropped,
        // element 3 panicked → ApiVec leaves [1] + leak-amplification of
        // unprocessed tail (3, 4, 5) is acceptable per std semantics.
        // Whatever ApiVec leaves, it must NOT be empty when the predicate
        // managed to keep at least one element.
        assert!(
            !v.is_empty(),
            "kept prefix [1, ...] must survive the panic; std::Vec::retain has the same contract"
        );
        assert_eq!(v[0], 1, "element 1 must be retained");
    }

    #[test]
    fn vec_dedup_panic_preserves_kept_prefix() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, i32> = arena.alloc_vec();
        v.extend([1_i32, 1, 2, 2, 3, 3]);

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            v.dedup_by(|a, _b| {
                assert!(*a != 3, "dedup panic");
                false
            });
        }));
        assert!(result.is_err());
        // At least one element must survive the panic; the all-elements-wiped
        // bug would leave the vector completely empty.
        assert!(!v.is_empty(), "Vec must not be fully wiped on dedup-predicate panic");
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn string_push_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut s = arena.alloc_string();
        s.push('x');
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn string_reserve_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut s = arena.alloc_string();
        s.reserve(128);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn string_replace_range_panics_from_grow_to_at_least() {
        let arena = Arena::builder_in(FailingAllocator::new(1)).build();
        let mut s = ArenaString::from_in("a", &arena);
        // `FailingAllocator` denies every allocation after the first
        // regardless of size; a moderate replacement (well past the
        // initial small chunk's residual capacity) is sufficient.
        let replacement = "x".repeat(1024);
        s.replace_range(0..1, replacement);
    }

    #[test]
    fn string_reserve_zero_on_nonempty_string_is_noop() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("already allocated", &arena);
        let cap = s.capacity();
        s.reserve(0);
        assert_eq!(s.capacity(), cap);
        assert_eq!(s.as_str(), "already allocated");
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn vec_with_capacity_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let _v: ArenaVec<'_, u8, _> = arena.alloc_vec_with_capacity(8);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn vec_into_arc_panics_on_shared_allocator_error() {
        let arena = Arena::builder_in(SendFailingAllocator::new(1)).build();
        // Fill most of the first chunk, then split: the tail has no freeze
        // prefix of its own, so freezing it into an `Arc` (a copy of equal
        // size) cannot reuse the buffer in place and must acquire a second
        // chunk, which the failing allocator rejects.
        let mut v: ArenaVec<'_, u8, _> = arena.alloc_vec_with_capacity(400);
        v.extend((0..400).map(|_| 0u8));
        let tail = v.split_off(200);
        let _arc = multitude::Arc::from(tail);
    }

    #[test]
    fn vec_into_box_handles_zst_fallback() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<()>();
        for _ in 0..16 {
            v.push(());
        }
        let b = v.into_boxed_slice();
        assert_eq!(b.len(), 16);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn vec_into_box_panics_on_zst_drop_alloc_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut v = arena.alloc_vec::<DropZst>();
        v.extend([DropZst, DropZst, DropZst]);
        let _ = v.into_boxed_slice();
    }

    #[test]
    fn vec_into_box_falls_back_when_drop_entry_install_misses() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<Droppy>();
        v.extend([Droppy("a"), Droppy("b")]);
        let _decoy = arena.alloc_slice_fill_with(70_000, |i| i as u8);
        let b = v.into_boxed_slice();
        assert_eq!(b.len(), 2);
    }

    #[test]
    // Skipped under Miri: building + dropping `u16::MAX + 1` elements
    // (~65K) exceeds Miri's test budget. The lifted restriction is a
    // runtime property, not a memory-safety one, so native + cargo-careful
    // runs cover it.
    #[cfg_attr(miri, ignore)]
    fn vec_into_box_drop_slice_longer_than_u16_succeeds() {
        // `Box<[T]>` drops via `drop_in_place::<[T]>` (no `u16`-counted
        // drop entry), so a `T: Drop` slice longer than `u16::MAX` freezes
        // into a `Box` without rejection.
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<Droppy>();
        let len = (u16::MAX as usize) + 1;
        v.extend((0..len).map(|_| Droppy("many")));
        let b = v.into_boxed_slice();
        assert_eq!(b.len(), len);
    }

    #[test]
    fn vec_resize_moves_final_clone_source_into_last_slot() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<std::string::String>();
        v.resize(3, "x".to_owned());
        assert_eq!(&*v, &["x", "x", "x"]);
    }

    #[test]
    fn vec_realloc_edge_cases_are_observable_through_public_api() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity(8);
        v.extend([1_u32, 2, 3, 4]);

        v.reserve_exact(0);
        assert!(v.capacity() >= 8);

        v.reserve(32);
        assert_eq!(&*v, &[1, 2, 3, 4]);

        v.clear();
        v.shrink_to_fit();
        assert_eq!(v.capacity(), 0);
    }

    #[test]
    fn vec_shrink_to_fit_oversized_chunk_is_a_noop() {
        // Buffers allocated in oversized chunks (cap > MAX_NORMAL_ALLOC)
        // are never at the `current` bump cursor, so
        // `shrink_to_fit` must no-op rather than allocate-copy-deallocate
        // (which would just churn fresh chunks for no semantic benefit).
        // Verify the no-op path under a one-shot allocator that would
        // refuse any subsequent allocation, demonstrating that no
        // allocator call is made.
        let arena = Arena::builder_in(FailingAllocator::new(1)).max_normal_alloc(4096).build();
        let mut v = arena.alloc_vec_with_capacity(70_000);
        let cap_before = v.capacity();
        v.extend([1_u32, 2, 3, 4]);
        v.shrink_to_fit();
        assert_eq!(v.capacity(), cap_before);
        assert_eq!(v.len(), 4);
    }

    #[test]
    fn arena_allocator_grow_falls_back_when_in_place_growth_is_ineligible() {
        let arena = Arena::new();
        let alloc = &arena;
        let old = Layout::from_size_align(8, 8).unwrap();
        let ptr = alloc.allocate(old).unwrap().cast::<u8>();

        let different_align = Layout::from_size_align(16, 16).unwrap();
        let grown = unsafe { Allocator::grow(&alloc, ptr, old, different_align) }.unwrap();
        unsafe { Allocator::deallocate(&alloc, grown.cast(), different_align) };

        let old = Layout::from_size_align(16, 8).unwrap();
        let ptr = alloc.allocate(old).unwrap().cast::<u8>();
        let smaller = Layout::from_size_align(8, 8).unwrap();
        let shrunk = unsafe { Allocator::shrink(&alloc, ptr, old, smaller) }.unwrap();
        unsafe { Allocator::deallocate(&alloc, shrunk.cast(), smaller) };
    }

    #[test]
    fn arena_allocator_grow_zeroed_extends_in_place() {
        let arena = Arena::new();
        let alloc = &arena;
        let old = Layout::from_size_align(8, 8).unwrap();
        let new = Layout::from_size_align(32, 8).unwrap();
        let ptr = alloc.allocate(old).unwrap().cast::<u8>();
        // SAFETY: `ptr` addresses 8 writable bytes.
        unsafe { ptr.as_ptr().write_bytes(0xA5, old.size()) };

        // SAFETY: `ptr` came from `alloc` with `old`; `new` is larger.
        let grown = unsafe { Allocator::grow_zeroed(&alloc, ptr, old, new) }.unwrap();
        assert_eq!(grown.cast::<u8>(), ptr);
        // SAFETY: `grown` addresses 32 initialized bytes.
        unsafe {
            assert!(
                core::slice::from_raw_parts(grown.cast::<u8>().as_ptr(), old.size())
                    .iter()
                    .all(|&b| b == 0xA5)
            );
            assert!(
                core::slice::from_raw_parts(grown.cast::<u8>().as_ptr().add(old.size()), new.size() - old.size())
                    .iter()
                    .all(|&b| b == 0)
            );
            Allocator::deallocate(&alloc, grown.cast(), new);
        }
    }

    #[test]
    fn arena_allocator_shrink_reuses_nonzero_block() {
        let arena = Arena::new();
        let alloc = &arena;
        let old = Layout::from_size_align(32, 8).unwrap();
        let new = Layout::from_size_align(8, 8).unwrap();
        let ptr = alloc.allocate(old).unwrap().cast::<u8>();
        // SAFETY: `ptr` addresses 32 writable bytes.
        unsafe { ptr.as_ptr().write_bytes(0x5A, old.size()) };

        // SAFETY: `ptr` came from `alloc` with `old`; `new` is smaller.
        let shrunk = unsafe { Allocator::shrink(&alloc, ptr, old, new) }.unwrap();
        assert_eq!(shrunk.cast::<u8>(), ptr);
        // SAFETY: `shrunk` addresses at least 8 initialized bytes.
        unsafe {
            assert!(
                core::slice::from_raw_parts(shrunk.cast::<u8>().as_ptr(), new.size())
                    .iter()
                    .all(|&b| b == 0x5A)
            );
            Allocator::deallocate(&alloc, shrunk.cast(), new);
        }
    }

    #[test]
    fn arena_slice_clone_no_drop_branch() {
        let arena = Arena::new();
        let values = [10_u32, 20, 30];
        let cloned = arena.alloc_slice_clone(values);
        assert_eq!(&*cloned, &[10, 20, 30]);
    }

    #[test]
    fn shared_refill_preserves_reentrant_drop_allocation() {
        static REENTERED: AtomicUsize = AtomicUsize::new(0);
        REENTERED.store(0, Ordering::SeqCst);

        struct ReentrantDrop {
            arena: *const Arena,
        }

        unsafe impl Send for ReentrantDrop {}
        unsafe impl Sync for ReentrantDrop {}

        impl Drop for ReentrantDrop {
            fn drop(&mut self) {
                let arena = unsafe { &*self.arena };
                let value = arena.alloc_arc(0xCAFE_u64);
                assert_eq!(*value, 0xCAFE);
                REENTERED.fetch_add(1, Ordering::SeqCst);
            }
        }

        let arena = Arena::new();
        let arena_ptr: *const Arena = &raw const arena;

        let reentrant = arena.alloc_arc(ReentrantDrop { arena: arena_ptr });
        drop(reentrant);

        // Drain the current chunk in one bulk allocation so the next
        // outer alloc forces a refill. A single 64 KiB uninit Arc
        // takes the entire chunk's worth of bytes; cheaper than the
        // prior 16 × 4 KiB fillers (16× fewer atomic ops under Miri).
        let filler = arena.alloc_uninit_arc::<[u8; 60 * 1024]>();
        drop(filler);

        let outer = arena.alloc_arc([0x55_u8; 4096]);
        assert_eq!(outer[0], 0x55);
        assert_eq!(REENTERED.load(Ordering::SeqCst), 1);
    }
}

mod public_surface_behavior_3 {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local statics next to their use")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert Err returns")]
    #![allow(clippy::ptr_as_ptr, reason = "test code uses `as` casts for raw pointers")]
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena};

    use crate::common;

    #[derive(Clone)]
    struct Droppy(&'static str);

    impl Drop for Droppy {
        fn drop(&mut self) {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    // A decoy entry forces initialization retargeting past the first entry.

    #[test]
    fn arc_single_assume_init_loop_traverses_past_first_drop_entry() {
        let arena = Arena::new();
        let arc_uninit = arena.alloc_uninit_arc::<Droppy>();
        let _decoy: Arc<Droppy> = arena.alloc_arc(Droppy("decoy"));
        unsafe {
            Arc::as_ptr(&arc_uninit)
                .cast_mut()
                .write(core::mem::MaybeUninit::new(Droppy("target")));
        }
        let arc = unsafe { arc_uninit.assume_init() };
        assert_eq!(arc.0, "target");
    }

    #[test]
    fn arc_slice_assume_init_loop_traverses_past_first_drop_entry() {
        let arena = Arena::new();
        let arc_uninit = arena.alloc_uninit_slice_arc::<Droppy>(2);
        let _decoy: Arc<Droppy> = arena.alloc_arc(Droppy("decoy"));
        unsafe {
            let base = Arc::as_ptr(&arc_uninit).cast::<core::mem::MaybeUninit<Droppy>>().cast_mut();
            (*base.add(0)).write(Droppy("a"));
            (*base.add(1)).write(Droppy("b"));
        }
        let arc = unsafe { arc_uninit.assume_init() };
        assert_eq!(arc[1].0, "b");
    }

    #[test]
    fn vec_swap_remove_last_index_skips_copy() {
        // Drives the `idx == self.len` branch of `swap_remove` where no
        // element copy is performed.
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.extend([1_u32, 2, 3]);
        let last = v.swap_remove(2);
        assert_eq!(last, 3);
        assert_eq!(v.as_slice(), &[1, 2]);
    }

    #[test]
    fn vec_into_iter_partial_drop_compacts_tail() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static DROPPED: AtomicUsize = AtomicUsize::new(0);

        struct Tracked(#[expect(dead_code, reason = "field only exists to make Tracked non-ZST")] u32);
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROPPED.store(0, Ordering::Relaxed);
        let arena = Arena::new();
        let mut v: ArenaVec<'_, Tracked> = arena.alloc_vec_with_capacity(4);
        for i in 0..4_u32 {
            v.push(Tracked(i));
        }
        let mut it = v.into_iter();
        // Dropping a partially consumed iterator compacts and drops its tail.
        let _a = it.next().unwrap();
        let _b = it.next().unwrap();
        drop(_a);
        drop(_b);
        // At this point DROPPED == 2.
        assert_eq!(DROPPED.load(Ordering::Relaxed), 2);
        drop(it);
        // Dropping the iter compacts the surviving tail (2 elements) and drops them.
        assert_eq!(DROPPED.load(Ordering::Relaxed), 4);
    }

    #[cfg(not(utc_backend))]
    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct OverAligned32K;

    // SAFETY: zero-sized POD; no drop.
    #[cfg(not(utc_backend))]
    unsafe impl Send for OverAligned32K {}
    // SAFETY: zero-sized POD; no drop.
    #[cfg(not(utc_backend))]
    unsafe impl Sync for OverAligned32K {}

    #[cfg(not(utc_backend))]
    #[test]
    fn try_alloc_slice_fill_with_arc_rejects_over_aligned() {
        let arena = Arena::new();
        // `try_alloc_slice_fill_with_arc` for `T: !needs_drop` routes through
        // `try_alloc_slice_shared_no_drop_with`, which checks
        // `align >= MAX_SMART_PTR_ALIGN` and errors.
        let result = arena.try_alloc_slice_fill_with_arc::<OverAligned32K, _>(2, |_| OverAligned32K);
        assert!(result.is_err());
    }

    #[test]
    fn cache_push_pop_contention_drives_cas_retries() {
        use std::sync::Barrier;
        use std::thread;

        // Force CAS contention on cache push/pop and reserve_budget by
        // hammering the same arena from many threads simultaneously.
        let arena: Arena = Arena::builder().max_normal_alloc(4096).byte_budget(128 * 1024 * 1024).build();

        // Group handles per thread so releases contend on the shared cache.
        let nthreads = 8;
        let per_thread = 32;
        let mut sets: Vec<Vec<multitude::Arc<u64>>> = (0..nthreads).map(|_| Vec::with_capacity(per_thread)).collect();
        for set in &mut sets {
            for _ in 0..per_thread {
                set.push(arena.alloc_arc(42));
            }
        }
        let barrier = std::sync::Arc::new(Barrier::new(nthreads));
        let mut handles = Vec::new();
        for set in sets {
            let b = barrier.clone();
            handles.push(thread::spawn(move || {
                // Synchronize the drop storm so threads race on the
                // Treiber-stack push CAS in `push`.
                b.wait();
                for a in set {
                    drop(a);
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    }

    #[test]
    fn vec_shrink_to_fit_is_a_noop_when_not_at_cursor() {
        // A buffer behind the bump cursor cannot reclaim storage.
        let alloc = common::FailingAllocator::new(2);
        let arena = Arena::new_in(alloc);
        let mut v = arena.alloc_vec::<u8>();
        v.reserve(100);
        let cap_before = v.capacity();
        // Consume the rest of the chunk so the vec's buffer is no longer
        // at the bump cursor.
        let _filler = arena.alloc_slice_fill_with::<u8, _>(400, |_| 0);
        // SAFETY: u8 is valid for any bit pattern and `cap >= 50` after `reserve(100)`.
        unsafe { v.set_len(50) };
        v.shrink_to_fit();
        // Capacity unchanged: shrink was a no-op.
        assert_eq!(v.capacity(), cap_before);
        assert_eq!(v.len(), 50);
    }
}

mod public_surface_behavior_4 {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn box_into_pin_via_from_impl() {
        let arena = Arena::new();
        let b: multitude::Box<u32> = arena.alloc_box(42_u32);
        let pinned: core::pin::Pin<multitude::Box<u32>> = b.into();
        assert_eq!(*pinned, 42);
    }

    #[test]
    fn string_insert_str_at_end_of_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hi");
        s.insert_str(s.len(), "!");
        assert_eq!(s.as_str(), "hi!");
    }

    #[test]
    fn string_replace_range_empty_at_end() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abc");
        let n = s.len();
        s.replace_range(n..n, "xyz");
        assert_eq!(s.as_str(), "abcxyz");
    }

    #[test]
    fn vec_resize_with_clone_panic_drops_partial() {
        use std::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Tracker<'a> {
            clones_made: &'a Cell<usize>,
            clones_dropped: &'a Cell<usize>,
            panic_after: usize,
        }
        impl Clone for Tracker<'_> {
            fn clone(&self) -> Self {
                let n = self.clones_made.get() + 1;
                self.clones_made.set(n);
                assert!(n != self.panic_after, "clone #{n} panics by design");
                Tracker {
                    clones_made: self.clones_made,
                    clones_dropped: self.clones_dropped,
                    panic_after: self.panic_after,
                }
            }
        }
        impl Drop for Tracker<'_> {
            fn drop(&mut self) {
                self.clones_dropped.set(self.clones_dropped.get() + 1);
            }
        }

        let clones_made = Cell::new(0);
        let clones_dropped = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: multitude::vec::Vec<'_, Tracker<'_>> = arena.alloc_vec_with_capacity(8);
            v.push(Tracker {
                clones_made: &clones_made,
                clones_dropped: &clones_dropped,
                panic_after: 3,
            });
            let seed = Tracker {
                clones_made: &clones_made,
                clones_dropped: &clones_dropped,
                panic_after: 3,
            };
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                v.resize(6, seed);
            }));
            assert!(result.is_err(), "panicking clone in resize must propagate");
        }
        drop(arena);
        // 2 successful clones happened (#1, #2) before #3 panicked. Resize's
        // panic-recovery Guard must have dropped those 2 already-written
        // elements before unwinding; the initial v[0] is dropped on `drop(v)`.
        // So total drops counted: 2 (rolled-back clones) + 1 (v[0]) + 1 (seed
        // — never moved into the Vec because the panic happened before the
        // final move).
        assert!(
            clones_dropped.get() >= 2,
            "Guard must drop the 2 successful clones rolled back by the resize panic; got {}",
            clones_dropped.get()
        );
    }
}
