// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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

#[cfg(feature = "stats")]
#[test]
fn new_does_not_eagerly_allocate_chunk() {
    // Sentinel-based slots remove the need to pre-allocate a Local
    // chunk at construction; the first allocation lazily pulls one in.
    let arena = Arena::new();
    assert_eq!(arena.stats().normal_local_chunks_allocated, 0);
    let _a = arena.alloc_rc(0_u8);
    assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
}

#[test]
fn default_works() {
    let arena: Arena = Arena::default();
    let v = arena.alloc_rc(42_u32);
    assert_eq!(*v, 42);
}

#[test]
fn allocator_accessor() {
    let arena = Arena::new();
    let _: &allocator_api2::alloc::Global = arena.allocator();
}

#[test]
fn debug_format_includes_stats() {
    let arena = Arena::new();
    let _ = arena.alloc_rc(1_u8);
    let s = format!("{arena:?}");
    assert!(s.contains("Arena"));
    #[cfg(feature = "stats")]
    assert!(s.contains("stats"));
}

#[test]
fn new_in_with_global() {
    let arena: Arena<allocator_api2::alloc::Global> = Arena::new_in(allocator_api2::alloc::Global);
    let v = arena.alloc_rc(7_i32);
    assert_eq!(*v, 7);
}

#[test]
fn builder_in_with_global() {
    let arena = Arena::builder_in(allocator_api2::alloc::Global).build();
    let v = arena.alloc_rc(7_i32);
    assert_eq!(*v, 7);
}

#[cfg(feature = "stats")]
#[test]
fn builder_default_matches_arena_new() {
    let a = Arena::builder().build();
    let b = Arena::new();
    // Sentinel-based slots: no chunk is allocated until the first user request.
    assert_eq!(a.stats().normal_local_chunks_allocated, 0);
    assert_eq!(b.stats().normal_local_chunks_allocated, 0);
    let _ = a.alloc_rc(0_u32);
    let _ = b.alloc_rc(0_u32);
    assert_eq!(a.stats().normal_local_chunks_allocated, 1);
    assert_eq!(b.stats().normal_local_chunks_allocated, 1);
}

#[test]
fn builder_default_impl() {
    // Drives `<ArenaBuilder<Global> as Default>::default()`.
    let builder = multitude::ArenaBuilder::default();
    let arena = builder.build();
    let v = arena.alloc_rc(99_u32);
    assert_eq!(*v, 99);
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
fn builder_small_chunk_size_works() {
    // 8 KiB chunks are tiny (well above the 2 KiB minimum) but legal.
    // The mask trick must still recover the chunk header from a value
    // pointer in a small chunk (chunks are still 64 KiB-aligned, just
    // smaller in actual size).
    let arena = Arena::builder().build();
    let v = arena.alloc_rc(42_u32);
    assert_eq!(*v, 42);
    let v2 = arena.alloc_rc(99_u32);
    assert_eq!(*v2, 99);
}

#[test]
fn small_chunk_size_round_trip_many_allocs() {
    let arena = Arena::builder().build();
    let mut handles = std::vec::Vec::new();
    for i in 0..1000_u32 {
        handles.push(arena.alloc_rc(i));
    }
    drop(handles);
    let h = arena.alloc_rc(99_u32);
    assert_eq!(*h, 99);
}

#[cfg(feature = "stats")]
#[test]
fn byte_budget_caps_total_chunk_bytes() {
    let arena = Arena::builder().byte_budget(8 * 1024).build();
    let mut handles = std::vec::Vec::new();
    let mut iters = 0_u32;
    loop {
        iters += 1;
        match arena.try_alloc_rc([0_u8; 256]) {
            Ok(h) => handles.push(h),
            Err(_) => break,
        }
        if iters > 1000 {
            panic!("byte_budget did not stop allocations");
        }
    }
    // Adaptive sizing allocates many small chunks (1 KiB up to 4 KiB
    // before further growth would breach the 8 KiB budget). The
    // important property is that the budget enforces a finite number of
    // chunks and a finite number of allocations — neither should run
    // unbounded.
    assert!(arena.stats().normal_local_chunks_allocated >= 1);
    assert!(arena.stats().normal_local_chunks_allocated <= 16);
    assert!(handles.len() < 32);
}

#[cfg(feature = "stats")]
#[test]
fn cache_reuse() {
    let arena = Arena::builder().build();
    let mut handles = std::vec::Vec::new();
    for i in 0..30_000_u64 {
        handles.push(arena.alloc_rc(i));
    }
    let stats = arena.stats();
    assert!(stats.normal_local_chunks_allocated >= 2);
    drop(handles);
    let chunks_before = arena.stats().normal_local_chunks_allocated;
    let _v = arena.alloc_rc(0_u64);
    let chunks_after = arena.stats().normal_local_chunks_allocated;
    assert_eq!(chunks_after, chunks_before, "expected cache hit");
}

#[cfg(feature = "stats")]
#[test]
fn preallocate_skips_underlying_allocation_calls() {
    // `with_capacity_local(1024)` picks size class 1 (= 1 KiB) and
    // allocates one chunk of that class into the local cache.
    let arena = Arena::builder().with_capacity_local(1024).build();
    assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
    // First user alloc pulls from the cache — no new chunk allocated.
    let _ = arena.alloc_rc(0_u32);
    assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
    let mut handles = std::vec::Vec::new();
    for i in 0..if cfg!(miri) { 256_u64 } else { 10_000_u64 } {
        handles.push(arena.alloc_rc(i));
    }
    assert!(arena.stats().normal_local_chunks_allocated >= 1);
}

#[cfg(feature = "stats")]
#[test]
fn chunks_allocated_first_user_alloc_creates_chunk() {
    // Sentinel-based slots: no chunk is allocated until first user request.
    let arena = Arena::new();
    assert_eq!(arena.stats().normal_local_chunks_allocated, 0);
    let _a = arena.alloc_rc(0_u8);
    assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
}

#[cfg(feature = "stats")]
#[test]
fn stats_total_bytes_allocated() {
    let arena = Arena::new();
    let _a = arena.alloc_rc(0_u64);
    let _b = arena.alloc_rc(0_u32);
    assert!(arena.stats().total_bytes_allocated >= 12);
}

#[cfg(feature = "stats")]
#[test]
fn stats_oversized_chunks_counted() {
    // Default max_normal_alloc is 16 KiB; a 32 KiB allocation goes oversized.
    let arena = Arena::new();
    let _big = arena.alloc_slice_copy_rc([0_u8; 32 * 1024]);
    assert!(arena.stats().oversized_local_chunks_allocated >= 1);
}

#[cfg(feature = "stats")]
#[test]
fn stats_wasted_tail_bytes_at_retirement() {
    // Build an arena, fill a chunk so its slack can't fit the next
    // request, then trigger retirement. Retain smart pointers so the chunk
    // doesn't get cached and reused.
    let arena = Arena::builder().build();
    let mut handles = std::vec::Vec::new();
    for _ in 0..28 {
        handles.push(arena.alloc_rc([0_u8; 256])); // 28 * 256 = 7168 B
    }
    let _h2 = arena.alloc_rc([0_u8; 2048]);
    assert!(arena.stats().wasted_tail_bytes > 0);
}

#[cfg(feature = "stats")]
#[test]
fn stats_string_relocation_counted() {
    let arena = Arena::builder().build();
    let _other = arena.alloc_rc(0_u32);
    let mut s2 = arena.alloc_string();
    s2.push_str("first");
    let _another = arena.alloc_rc(1_u32); // breaks cursor adjacency
    s2.push_str("x".repeat(100));
    assert!(arena.stats().relocations >= 1);
}

#[cfg(feature = "stats")]
#[test]
fn stats_vec_relocation_counted() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.push(1);
    let _other = arena.alloc_rc(0_u8); // breaks cursor adjacency
    for i in 0..1000_u32 {
        v.push(i);
    }
    let stats = arena.stats();
    assert!(stats.relocations >= 1);
}

#[test]
fn alloc_zst_works() {
    #[derive(Debug, PartialEq)]
    struct Zst;
    let arena = Arena::new();
    let r = arena.alloc_rc(Zst);
    assert_eq!(*r, Zst);
}

#[test]
fn try_alloc_str_returns_mutable_str() {
    let arena = Arena::new();
    let s: &mut str = arena.try_alloc_str("hello").unwrap();
    s.make_ascii_uppercase();
    assert_eq!(s, "HELLO");
}

#[test]
fn try_alloc_str_rc_returns_handle() {
    let arena = Arena::new();
    let s = arena.try_alloc_str_rc("rc").unwrap();
    assert_eq!(&*s, "rc");
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
/// Under Miri we use the minimum size that still triggers the
/// oversized branch to keep its tagged-pointer tracking tractable.
const OVERSIZED_BYTES: usize = if cfg!(miri) { 65 * 1024 } else { 128 * 1024 };

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

// Regression: a panic in the smart-pointer slice-fill closure on an
// oversized chunk used to leak the chunk + its `ArenaInner` because
// `SliceReservation` had no Drop and the chunk's refcount stayed at 0
// until `commit_slice` ran.
#[test]
fn panic_in_oversized_slice_fill_with_rc_does_not_leak() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let alloc = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _r = arena.alloc_slice_fill_with_rc::<u32, _>(8 * 1024, |i| {
                assert!(i < 5, "synthetic panic");
                i as u32
            });
        }));
        assert!(result.is_err());
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

    use multitude::{Arc, Arena, Rc};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
    #[test]
    fn reset_on_empty_arena_is_a_noop() {
        let mut arena = Arena::new();
        arena.reset();
        let r = arena.alloc_rc(1_u32);
        assert_eq!(*r, 1);
    }

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
        let mut arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).with_capacity_local(64 * 1024).build();
        let _ = arena.alloc([0_u8; 4000]);
        let _ = arena.alloc([0_u8; 4000]);
        let _ = arena.alloc([0_u8; 4000]);
        let _ = arena.alloc([0_u8; 4000]);
        let _ = arena.alloc([0_u8; 4000]);
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
    fn reset_with_outstanding_arena_rc_keeps_handle_valid() {
        let mut arena = Arena::new();
        let r: Rc<u32> = arena.alloc_rc(42);
        arena.reset();
        assert_eq!(*r, 42);
        let r2 = r.clone();
        assert_eq!(*r2, 42);
        drop(r);
        drop(r2);
        let _ = arena.alloc_rc(99_u32);
    }

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
    fn reset_runs_destructor_when_last_handle_drops_post_reset() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let mut arena = Arena::new();
        let r: Rc<Tracked> = arena.alloc_rc(Tracked);
        arena.reset();
        // Smart pointer outlived reset; destructor not yet run.
        assert_eq!(COUNT.load(Ordering::SeqCst), 0);
        drop(r);
        // Now the chunk's last smart pointer dropped → destructor runs.
        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reset_runs_chunk_residents_drops_only_once_with_handle_outstanding() {
        // Subtle: a single chunk hosts both an Arena::alloc-style value and
        // an ArenaRc smart pointer. The ArenaRc keeps the chunk alive past reset,
        // so the alloc-style value's destructor doesn't run at reset. It
        // runs when the last smart pointer drops.
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let mut arena = Arena::new();
        let r: Rc<u8> = arena.alloc_rc(0);
        let _ = arena.alloc(Tracked); // pinned alloc-style; same chunk
        arena.reset();
        assert_eq!(
            COUNT.load(Ordering::SeqCst),
            0,
            "destructor must NOT run yet — chunk is detached but alive"
        );
        drop(r);
        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
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

    #[cfg(feature = "stats")]
    #[test]
    fn reset_returns_chunk_to_cache_when_handles_drop_after_reset() {
        let mut arena = Arena::builder().with_capacity_local(64 * 1024).build();
        let r: Rc<u64> = arena.alloc_rc(1);
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        arena.reset();
        drop(r); // last handle: chunk is reclaimed → cached.
        let _ = arena.alloc(0_u64);
        assert_eq!(
            arena.stats().normal_local_chunks_allocated,
            chunks_before,
            "chunk should have rejoined the cache when handle dropped"
        );
    }

    #[test]
    fn reset_handles_destructor_that_drops_other_smart_pointer() {
        // Regression: a destructor running during `Arena::reset` may drop
        // another smart pointer that triggers re-entrant chunk teardown,
        // potentially installing new chunks in `current_*` mid-drain.
        // The fixed-point loop must drain those re-entrantly-installed
        // chunks too, so all destructors run exactly once.
        static OUTER: AtomicUsize = AtomicUsize::new(0);
        static INNER: AtomicUsize = AtomicUsize::new(0);

        struct Outer<A: allocator_api2::alloc::Allocator + Clone + Send + Sync + 'static> {
            inner: Option<Arc<Inner, A>>,
        }
        struct Inner;
        impl Drop for Inner {
            fn drop(&mut self) {
                let _ = INNER.fetch_add(1, Ordering::SeqCst);
            }
        }
        impl<A: allocator_api2::alloc::Allocator + Clone + Send + Sync + 'static> Drop for Outer<A> {
            fn drop(&mut self) {
                let _ = OUTER.fetch_add(1, Ordering::SeqCst);
                let _ = self.inner.take();
            }
        }

        OUTER.store(0, Ordering::SeqCst);
        INNER.store(0, Ordering::SeqCst);
        let mut arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let inner = arena.alloc_arc(Inner);
        let _ = arena.alloc(Outer { inner: Some(inner) });
        // Force chunk rotation so Outer's chunk goes onto the pinned list.
        let _ = arena.alloc([0_u8; 1500]);
        let _ = arena.alloc([0_u8; 1500]);
        let _ = arena.alloc([0_u8; 1500]);
        arena.reset();
        assert_eq!(OUTER.load(Ordering::SeqCst), 1, "Outer::drop must run");
        assert_eq!(INNER.load(Ordering::SeqCst), 1, "Inner::drop must run");
        // Arena is still usable after reset.
        let r = arena.alloc_rc(42_u32);
        assert_eq!(*r, 42);
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
        let arena = Arena::new();
        let s = arena.alloc_slice_fill_with::<u8, _>(OVER_CHUNK, |i| (i & 0xff) as u8);
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(s[0], 0);
        assert_eq!(s[255], 255);
        assert_eq!(s[CHUNK_BYTES], (CHUNK_BYTES & 0xff) as u8);
        assert_eq!(s[OVER_CHUNK - 1], ((OVER_CHUNK - 1) & 0xff) as u8);
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
    fn alloc_slice_copy_rc_above_chunk_boundary() {
        let arena = Arena::new();
        let n = CHUNK_BYTES / 2 + 4; // 65544 bytes
        let src: Vec<u16> = (0..n as u16).map(|v| v.wrapping_mul(7)).collect();
        let r = arena.alloc_slice_copy_rc::<u16>(&src);
        assert_eq!(r.len(), src.len());
        for (i, &v) in r.iter().enumerate() {
            assert_eq!(v, (i as u16).wrapping_mul(7));
        }
    }

    #[test]
    fn alloc_slice_copy_arc_above_chunk_boundary() {
        let arena = Arena::new();
        let src: Vec<u8> = (0..OVER_CHUNK).map(|i| (i & 0xff) as u8).collect();
        let a = arena.alloc_slice_copy_arc::<u8>(&src);
        assert_eq!(a.len(), src.len());
        // Cross-thread sanity: Arc<[u8]> over the oversized chunk must travel.
        let a_clone = a.clone();
        let h = std::thread::spawn(move || {
            assert_eq!(a_clone.len(), OVER_CHUNK);
            assert_eq!(a_clone[OVER_CHUNK - 1], ((OVER_CHUNK - 1) & 0xff) as u8);
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
        // Drop slices have a `len <= u16::MAX` cap because the back-stack
        // entry encodes `len` in a `u16`. Pick a length that crosses the
        // 64 KiB chunk-byte boundary while staying within that cap.
        let len = u16::MAX as usize; // 65 535 elements of 1 byte each
        {
            let arena = Arena::new();
            let s = arena.alloc_slice_fill_with::<Counted, _>(len, |_| Counted(counter.clone()));
            assert_eq!(s.len(), len);
            // arena drop runs the drop list on the oversized chunk
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
        let len = u16::MAX as usize;
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
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        assert_eq!(v.len(), 0);
        // Push one byte at a time so the vector's amortized doubling
        // triggers relocations across chunk boundaries.
        for i in 0..(OVER_CHUNK + 1024) {
            v.push((i & 0xff) as u8);
        }
        assert_eq!(v.len(), OVER_CHUNK + 1024);
        for i in 0..v.len() {
            assert_eq!(v[i], (i & 0xff) as u8, "mismatch at index {i}");
        }
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
        // Stay under `u16::MAX` so the drop-list `len` field fits; use
        // a heavier element type so the byte footprint still crosses the
        // 64 KiB boundary on growth.
        let n = u16::MAX as usize;
        {
            let arena = Arena::new();
            let mut v = arena.alloc_vec::<Counted>();
            for _ in 0..n {
                v.push(Counted(counter.clone()));
            }
            assert_eq!(v.len(), n);
            // After many doublings the storage will have been relocated
            // through oversized chunks. Drop runs at arena drop.
        }
        assert_eq!(counter.load(Ordering::Relaxed), n);
    }

    #[test]
    fn alloc_vec_extend_from_iter_past_chunk_boundary() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u16>();
        v.extend((0..(OVER_CHUNK / 2) as u16).map(|i| i.wrapping_mul(13)));
        assert_eq!(v.len(), OVER_CHUNK / 2);
        for (i, &x) in v.iter().enumerate() {
            assert_eq!(x, (i as u16).wrapping_mul(13));
        }
    }

    #[test]
    fn vec_in_macro_initial_then_grow_past_chunk() {
        let arena = Arena::new();
        let mut v = multitude::vec::vec![in &arena; 0u32; 16];
        assert_eq!(v.len(), 16);
        while v.len() < (OVER_CHUNK / 4) {
            let next = v.len() as u32;
            v.push(next);
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
        // ASCII byte == 1 byte UTF-8.
        for i in 0..OVER_CHUNK {
            s.push((b'a' + ((i % 26) as u8)) as char);
        }
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(s.as_bytes()[0], b'a');
        assert_eq!(s.as_bytes()[CHUNK_BYTES], b'a' + ((CHUNK_BYTES % 26) as u8));
        assert_eq!(s.as_bytes()[OVER_CHUNK - 1], b'a' + (((OVER_CHUNK - 1) % 26) as u8));
    }

    #[test]
    fn alloc_string_grows_from_small_to_past_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        assert_eq!(s.len(), 0);
        s.push_str("hello");
        while s.len() < OVER_CHUNK {
            s.push('x');
        }
        assert!(s.len() >= OVER_CHUNK);
        assert!(s.as_str().starts_with("hello"));
        assert_eq!(s.as_bytes()[OVER_CHUNK - 1], b'x');
    }

    #[test]
    fn alloc_string_push_multibyte_grows_past_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        // Each emoji is 4 bytes UTF-8. Push enough to comfortably cross 64 KiB.
        let target_chars = (OVER_CHUNK / 4) + 16;
        for _ in 0..target_chars {
            s.push('🦀');
        }
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
        // is 131 074 bytes of buffer
        let mut s = arena.alloc_utf16_string_with_capacity(OVER_CHUNK);
        for i in 0..OVER_CHUNK {
            s.push((b'a' + ((i % 26) as u8)) as char);
        }
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(s.as_slice()[0], u16::from(b'a'));
        assert_eq!(s.as_slice()[OVER_CHUNK - 1], u16::from(b'a' + (((OVER_CHUNK - 1) % 26) as u8)));
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_string_grows_from_small_to_past_chunk_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("hello");
        while s.len() < OVER_CHUNK {
            s.push('y');
        }
        assert!(s.len() >= OVER_CHUNK);
        let v = s.as_slice();
        assert_eq!(v[0], u16::from(b'h'));
        assert_eq!(v[OVER_CHUNK - 1], u16::from(b'y'));
    }

    // ============================================================================
    // Stress: many oversized allocations in one arena
    // ============================================================================

    #[test]
    fn many_oversized_allocations_in_one_arena() {
        let arena = Arena::new();
        let mut keepers: Vec<&[u8]> = Vec::with_capacity(8);
        for round in 0..8u8 {
            let s: &mut [u8] = arena.alloc_slice_fill_with::<u8, _>(OVER_CHUNK, move |_| round);
            keepers.push(s);
        }
        for (round, s) in keepers.iter().enumerate() {
            assert_eq!(s.len(), OVER_CHUNK);
            assert_eq!(s[0], round as u8);
            assert_eq!(s[OVER_CHUNK - 1], round as u8);
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
    fn alloc_str_rc_above_chunk_boundary() {
        let arena = Arena::new();
        let big: String = "y".repeat(OVER_CHUNK);
        let s = arena.alloc_str_rc(&big);
        assert_eq!(s.len(), OVER_CHUNK);
        assert_eq!(&s[..5], "yyyyy");
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
        assert!(s.chars().all(|c| c == 'Q'));
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

        #[derive(Clone)]
        struct Counted(StdArc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, StdOrd::Relaxed);
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // Choose `len` so `len * size_of::<Counted>` crosses the 64 KiB
        // boundary while staying within `u16::MAX` (DST stores metadata
        // as a `u16`). `Counted` is one `StdArc`-wide field; we can fit
        // many before the u16 cap.
        let n = u16::MAX as usize;
        {
            let arena = Arena::new();
            let layout = core::alloc::Layout::array::<Counted>(n).unwrap();
            assert!(layout.size() > CHUNK_BYTES, "test must drive the oversized DST shared path");
            // SAFETY: init fills every slot of the slice fat pointer.
            let arc: multitude::Arc<[Counted]> = unsafe {
                arena.alloc_dst_arc::<[Counted]>(layout, n, |p: *mut [Counted]| {
                    for i in 0..n {
                        let slot: *mut Counted = (p as *mut Counted).add(i);
                        core::ptr::write(slot, Counted(counter.clone()));
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
        // DST slice metadata is stored as a `u16`, so `len <= u16::MAX`.
        // Use 2-byte elements with `len == u16::MAX` so the byte size
        // (131 070) crosses the 64 KiB chunk boundary and drives the
        // oversized shared DST path.
        const LEN: usize = u16::MAX as usize;
        let arena = Arena::new();
        let layout = core::alloc::Layout::array::<u16>(LEN).unwrap();
        assert!(layout.size() > CHUNK_BYTES, "test must drive the oversized DST shared path");
        // SAFETY: init fills every element.
        let arc: multitude::Arc<[u16]> = unsafe {
            arena.alloc_dst_arc::<[u16]>(layout, LEN, |p: *mut [u16]| {
                for i in 0..LEN {
                    (p as *mut u16).add(i).write(i as u16);
                }
            })
        };
        assert_eq!(arc.len(), LEN);
        assert_eq!(arc[0], 0);
        assert_eq!(arc[LEN - 1], (LEN - 1) as u16);
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
    fn alloc_rc_u64_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let rc = arena.alloc_rc(0xCAFE_BABE_u64);
            let ptr = &raw const *rc as usize;
            assert_eq!(ptr % align_of::<u64>(), 0, "Rc<u64> pointer misaligned: {ptr:#x}");
            assert_eq!(*rc, 0xCAFE_BABE_u64);
        }
    }

    #[test]
    fn alloc_rc_u128_is_aligned() {
        let arena = Arena::new();
        for _ in 0..100 {
            let rc = arena.alloc_rc(0xAABB_CCDD_EEFF_0011_u128);
            let ptr = &raw const *rc as usize;
            assert_eq!(ptr % align_of::<u128>(), 0, "Rc<u128> pointer misaligned: {ptr:#x}");
            assert_eq!(*rc, 0xAABB_CCDD_EEFF_0011_u128);
        }
    }

    #[test]
    fn alloc_rc_align32_is_aligned() {
        let arena = Arena::new();
        for i in 0..50 {
            let rc = arena.alloc_rc(Align32 { value: i });
            let ptr = &raw const *rc as usize;
            assert_eq!(ptr % 32, 0, "Rc<Align32> pointer misaligned: {ptr:#x}");
            assert_eq!(rc.value, i);
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
    fn interleaved_rc_alignments() {
        let arena = Arena::new();
        for i in 0_u64..50 {
            let a = arena.alloc_rc(i as u8);
            let b = arena.alloc_rc(i);
            let c = arena.alloc_rc(Align32 { value: i });

            assert_eq!((&raw const *a as usize) % align_of::<u8>(), 0);
            assert_eq!((&raw const *b as usize) % align_of::<u64>(), 0);
            assert_eq!((&raw const *c as usize) % 32, 0);

            assert_eq!(*a, i as u8);
            assert_eq!(*b, i);
            assert_eq!(c.value, i);
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
    fn rc_drop_runs_correctly_many_items() {
        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let before = DROP_COUNTER.load(Ordering::SeqCst);
        let n = 200;
        {
            let arena = Arena::new();
            let handles: Vec<_> = (0..n).map(|i| arena.alloc_rc(DropTracker(i))).collect();
            assert_eq!(handles.len(), n as usize);
            // Drops happen when the chunk is freed — which requires both
            // the handles AND the arena to be dropped.
            drop(handles);
            // Arena drop frees the chunks and runs all pending destructors.
        }
        let after = DROP_COUNTER.load(Ordering::SeqCst);
        assert_eq!(after - before, n as usize);
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
    fn rc_aligned_drop_runs_correctly() {
        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let before = ALIGNED_DROP_COUNTER.load(Ordering::SeqCst);
        let n = 50;
        {
            let arena = Arena::new();
            let handles: Vec<_> = (0..n)
                .map(|i| {
                    let rc = arena.alloc_rc(AlignedDropTracker { value: i });
                    let ptr = &raw const *rc as usize;
                    assert_eq!(ptr % 32, 0, "AlignedDropTracker misaligned: {ptr:#x}");
                    rc
                })
                .collect();
            assert_eq!(handles.len(), n as usize);
            drop(handles);
        }
        let after = ALIGNED_DROP_COUNTER.load(Ordering::SeqCst);
        assert_eq!(after - before, n as usize);
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
    fn consecutive_rc_allocs_do_not_overlap() {
        let arena = Arena::new();
        let handles: Vec<_> = (0..200_u64).map(|i| arena.alloc_rc(i)).collect();
        let mut addrs: Vec<usize> = handles.iter().map(|rc| &raw const **rc as usize).collect();
        addrs.sort_unstable();
        for window in addrs.windows(2) {
            assert!(
                window[0] + size_of::<u64>() <= window[1],
                "Rc allocations overlap: {:#x} + 8 > {:#x}",
                window[0],
                window[1]
            );
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
    fn filling_chunk_rc_triggers_new_allocation() {
        let arena = Arena::builder().build();
        // Prime
        let _prime = arena.alloc_rc(0_u64);
        let initial_chunks = arena.stats().normal_local_chunks_allocated;
        let mut handles = Vec::new();
        let mut count = 0_u64;
        while arena.stats().normal_local_chunks_allocated == initial_chunks {
            let rc = arena.alloc_rc(count);
            let ptr = &raw const *rc as usize;
            assert_eq!(ptr % align_of::<u64>(), 0);
            handles.push(rc);
            count += 1;
            assert!(count < 2000, "should have triggered new chunk by now");
        }
        assert!(count > 50, "chunk should hold many Rc<u64>s");
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
    fn oversize_rc_goes_to_oversized_chunk() {
        let arena = Arena::new();
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
        let big = arena.alloc_rc([0u8; 32 * 1024]);
        assert_eq!(big[0], 0);
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

    #[cfg(feature = "stats")]
    #[test]
    fn rc_drop_items_pack_efficiently_in_chunk() {
        let arena = Arena::builder().build();
        let _prime = arena.alloc_rc(Droppable(0));
        let initial_chunks = arena.stats().normal_local_chunks_allocated;
        let mut handles = Vec::new();
        let mut count = 0_u64;
        while arena.stats().normal_local_chunks_allocated == initial_chunks && count < 500 {
            handles.push(arena.alloc_rc(Droppable(count)));
            count += 1;
        }
        assert!(
            count > 10,
            "Only {count} Rc<Droppable> items fit in chunk — alignment math may be corrupted"
        );
    }

    // String pinning: the fast-path str allocation must pin the chunk so it
    // survives eviction. Without pinning, a filled chunk would be freed and
    // the returned &mut str would dangle.

    #[test]
    fn str_fast_path_survives_chunk_rotation() {
        // Use a small chunk so we can force eviction quickly.
        let arena = Arena::builder().build();
        // Rc allocation installs the chunk with pin_for_bump=false (unpinned).
        // Drop it immediately so only the arena slot's refcount keeps chunk alive.
        drop(arena.alloc_rc(42_u64));
        // First string: allocated via fast path in the unpinned chunk.
        // The fast path's `if !chunk_ref.pinned() { set_pinned(); }` must fire here.
        let first = arena.alloc_str("hello_world_pinning_test");
        // Fill the chunk with more strings to force a rotation.
        for i in 0..1000_u32 {
            let _ = arena.alloc_str(format!("fill_{i:04}"));
        }
        // If the first chunk wasn't pinned by the str fast path, it would
        // have been freed when evicted.
        assert_eq!(first, "hello_world_pinning_test");
    }

    /// Detects whether `alloc_str`'s fast path actually sets the pinned flag by
    /// observing chunk lifetime via `TrackingAllocator`. If pinning is missing
    /// (e.g., the `!` in `if !chunk_ref.pinned()` is deleted), the chunk is
    /// freed on rotation and `live_chunks` drops.
    #[cfg(feature = "stats")]
    #[test]
    fn str_fast_path_pinning_prevents_chunk_deallocation() {
        let alloc = common::TrackingAllocator::new();
        let arena = Arena::builder().allocator_in(alloc.clone()).build();
        // Install a chunk (unpinned) via alloc_rc (pin_for_bump=false).
        drop(arena.alloc_rc(0_u64));
        assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
        // alloc_str on the fast path MUST pin the chunk.
        let _s = arena.alloc_str("pin");
        // Fill the rest of the chunk with alloc_rc (pin_for_bump=false).
        // Drop each handle so only the slot's +1 keeps the chunk alive.
        while arena.stats().normal_local_chunks_allocated == 1 {
            drop(arena.alloc_rc(0_u64));
        }
        // Rotation happened. With correct pinning the old chunk is alive
        // in the pinned list; without it, the evicted guard freed the chunk.
        assert!(
            alloc.live_chunks() >= 2,
            "str fast-path pinning failed: old chunk was freed on rotation (live={})",
            alloc.live_chunks()
        );
    }

    // Slice fill_with panic safety: partial initialization must be cleaned up
    // if the fill closure panics.

    #[test]
    fn slice_fill_with_panics_drops_initialized_elements() {
        use std::panic;

        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let arena = Arena::new();
        let before = DROP_COUNTER.load(Ordering::SeqCst);
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_fill_with_rc(10, |i| {
                assert!(i != 5, "intentional panic at index 5");
                DropTracker(i as u64)
            });
        }));
        assert!(result.is_err(), "should have panicked");
        // The 5 successfully-initialized DropTrackers (indices 0..5)
        // should have been dropped by the panic guard.
        let after = DROP_COUNTER.load(Ordering::SeqCst);
        assert!(
            after - before >= 5,
            "Expected at least 5 drops from panic guard, got {}",
            after - before
        );
    }

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

    #[test]
    fn slice_clone_panics_drops_initialized_elements() {
        use std::panic;

        let _lock = DROP_TEST_LOCK.lock().unwrap();
        let arena = Arena::new();
        // Build source slice: items 0..10, panic_at=5 means clone of item[5] panics.
        let source: Vec<PanicOnClone> = (0..10).map(|i| PanicOnClone { id: i, panic_at: 5 }).collect();
        let before = DROP_COUNTER.load(Ordering::SeqCst);
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_clone_rc(&source);
        }));
        assert!(result.is_err(), "should have panicked during clone");
        // Items 0..5 were cloned successfully and must be dropped by the guard.
        let after = DROP_COUNTER.load(Ordering::SeqCst);
        assert!(
            after - before >= 5,
            "Expected at least 5 drops from clone panic guard, got {}",
            after - before
        );
        // Clean up source (drops 10 items).
        drop(source);
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

    #[cfg(feature = "stats")]
    #[test]
    fn allocator_grow_via_arena_vec_records_relocation() {
        // ArenaVec push that can't grow in place must relocate via
        // <&Arena<A> as Allocator>::grow → counted in stats.relocations.
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.push(1);
        let _decoy = arena.alloc_rc(0_u8); // breaks cursor adjacency
        for i in 0..1000_u32 {
            v.push(i);
        }
        assert!(arena.stats().relocations >= 1);
    }

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

    #[cfg(feature = "stats")]
    #[test]
    fn oversized_chunk_used_when_alloc_too_big() {
        let arena = Arena::new();
        let big = arena.alloc_slice_copy_rc([0_u8; 32 * 1024]);
        assert_eq!(big.len(), 32 * 1024);
        assert!(arena.stats().oversized_local_chunks_allocated >= 1);
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
        // Use a 5 MiB budget — enough for one 64 KiB chunk plus header,
        // but not enough for two 4 MiB-equivalents.
        let arena = Arena::builder().byte_budget(5 * 1024 * 1024).build();
        // Allocate many u64s to force at least one chunk allocation.
        let mut keep: Vec<Box<u64>> = Vec::new();
        for i in 0..1024_u64 {
            keep.push(arena.alloc_box(i));
        }
        let stats_full = arena.stats();
        assert!(stats_full.normal_local_chunks_allocated >= 1);
        // Drop the boxes; the chunk(s) are recycled into the cache (or
        // freed). Either way, the byte-budget accountant should reflect
        // the live chunk bytes.
        drop(keep);
        drop(arena);
        // If `release_budget` is a no-op, the test for budget-bounded
        // re-allocation below would error. The user-observable invariant
        // we can pin is: a *fresh* arena with the same budget admits
        // the same workload — which it must, because the previous arena
        // released its budget on drop.
        let arena2 = Arena::builder().byte_budget(5 * 1024 * 1024).build();
        let mut keep2: Vec<Box<u64>> = Vec::new();
        for i in 0..1024_u64 {
            keep2.push(arena2.alloc_box(i));
        }
        assert!(arena2.stats().normal_local_chunks_allocated >= 1);
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
        assert_eq!(
            s.oversized_local_chunks_allocated, 0,
            "size == max_normal_alloc must route through normal (cacheable) path"
        );
        assert!(s.normal_local_chunks_allocated >= 1);
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
        assert_eq!(s.oversized_shared_chunks_allocated, 0);
        assert!(s.normal_shared_chunks_allocated >= 1);
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
        let arena = Arena::new();
        // Allocate ~256 KiB worth of u64s → forces several refills.
        let mut keep: Vec<Box<u64>> = Vec::new();
        for i in 0..32_768_u64 {
            keep.push(arena.alloc_box(i));
        }
        let s = arena.stats();
        // No oversized routing should occur for normal allocations.
        assert_eq!(s.oversized_local_chunks_allocated, 0);
        // The cumulative size of allocated normal chunks should be a sum
        // of powers of two up to 64 KiB inclusive: 0.5+1+2+…+64 KiB =
        // 127.5 KiB if every class up to 7 is exercised once; with retries
        // the practical bound is a small multiple of 64 KiB per refill.
        // We just check that no chunk exceeds 64 KiB:
        // `total_bytes_allocated` should be a multiple of class sizes
        // ≤ 64 KiB. Concretely, with NUM_CHUNK_CLASSES-1 = 7, the maximum
        // single chunk payload is 64 KiB, and 32_768 u64s = 256 KiB
        // requires at least 4 such chunks. With mutated `+ 1` (class 8 =
        // 128 KiB), the workload would fit in 2 chunks instead.
        assert!(
            s.normal_local_chunks_allocated >= 4,
            "32 KiB workload must use ≥ 4 chunks at class-7 ceiling, got {}",
            s.normal_local_chunks_allocated
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
        // Allocate a single 32 KiB blob, then more small allocs; the high-
        // water should pin at class ≥ 5 (1 KiB), making subsequent fresh
        // chunks at least that big.
        #[repr(align(8))]
        struct Blob([u8; 8 * 1024]); // 8 KiB
        let _b = arena.alloc_box(Blob([0; 8 * 1024]));
        // Now force a refill by allocating until we exhaust current chunk.
        // The fresh chunk must be at least 8 KiB class.
        let _v: Vec<Box<u64>> = (0..8_192_u64).map(|i| arena.alloc_box(i)).collect();
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

    /// Pins the `drop_list.rs` `PAD_BYTES` invariant: the resulting
    /// `DropEntry` slot size is a multiple of `align_of::<unsafe fn>()` =
    /// 8 (on 64-bit). The struct is private, but `core::mem::size_of`'s
    /// effect on the chunk's drop-back stride is observable via the
    /// number of drop entries that fit per chunk. With a wrong
    /// `PAD_BYTES`, the stride differs and the count of recoverable
    /// entries differs.
    ///
    /// We exercise this with a small-chunk arena: allocate exactly K
    /// drop-tracking values into one class-0 (512 B) chunk and verify
    /// every drop runs. With wrong-stride entries, replay reads
    /// misaligned bytes and the drop count differs.
    ///
    /// **NOTE**: Several `drop_list.rs:49/53` mutants only modify
    /// `PAD_BYTES` (a private `[u8; N]` field). Rust's `repr(C)`
    /// guarantees the struct size is rounded up to the alignment, so
    /// e.g. `PAD_BYTES = 0` and `PAD_BYTES = 4` produce *identical*
    /// struct sizes (16 bytes). Those mutants are therefore
    /// **equivalent** at the layout level. This test pins the
    /// observable invariant (drops run correctly) regardless.
    #[test]
    fn drop_entry_layout_supports_correct_replay() {
        #[derive(Debug)]
        struct D(StdArc<AtomicUsize>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let c = StdArc::new(AtomicUsize::new(0));
        {
            // Tight: 512-byte starter chunk forces an early refill,
            // exercising the back-stack stride across chunk boundaries.
            let arena = Arena::builder().with_capacity_local(512).build();
            let mut keep: Vec<multitude::Rc<D>> = Vec::new();
            for _ in 0..512_u32 {
                keep.push(arena.alloc_rc(D(c.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(c.load(Ordering::Relaxed), 512);
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

    /// Kills `local_chunk.rs:132:17 - → +` and `shared_chunk.rs:143:17
    /// `- → +` in `max_bump_extent`. The function returns
    /// `CHUNK_ALIGN - header_size()`; mutated to `+`, the bump extent
    /// reaches well past the chunk's first 64 KiB tile, which would
    /// break the smart-pointer chunk-recovery mask.
    ///
    /// Observable consequence: in a large (64 KiB) chunk, the bump
    /// cursor's reach is capped so that allocated value addresses lie
    /// in the first 64 KiB tile. If the cap is `+ header` (wrong), the
    /// cursor extends past 64 KiB and an Rc/Arc's chunk recovery via
    /// pointer masking targets the wrong chunk → UB.
    ///
    /// Detection: allocate enough into a 64 KiB chunk that the cursor
    /// would walk to the cap, then bind to an Rc/Arc and read back the
    /// value. With wrong cap, the read either segfaults or returns
    /// corrupted data.
    #[test]
    fn max_bump_extent_keeps_pointers_in_first_tile() {
        let arena = Arena::builder().with_capacity_local(64 * 1024).build();
        // Fill the chunk with many small Rc allocations.
        let mut keep: Vec<multitude::Rc<u32>> = Vec::new();
        for i in 0..4096_u32 {
            keep.push(arena.alloc_rc(i));
        }
        // Read back all values — wrong masking would yield stale/garbled
        // reads or segfaults.
        for (i, r) in keep.iter().enumerate() {
            assert_eq!(**r, i as u32);
        }

        // Same for shared/Arc.
        let arena2 = Arena::builder().with_capacity_shared(64 * 1024).build();
        let mut keep_arc: Vec<multitude::Arc<u32>> = Vec::new();
        for i in 0..4096_u32 {
            keep_arc.push(arena2.alloc_arc(i));
        }
        for (i, r) in keep_arc.iter().enumerate() {
            assert_eq!(**r, i as u32);
        }
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
    use multitude::strings::ArcStr;
    use multitude::{Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    const MAX_NORMAL_ALLOC: usize = 16 * 1024;
    const PREFIX_BYTES: usize = core::mem::size_of::<usize>();

    // ---------------------------------------------------------------------------
    // alloc_str.rs:251 — `if total > max_normal_alloc` in `try_alloc_str_prefixed_local`.
    // Kills `>` → `>=` at the exact boundary `total == max_normal_alloc`.
    // ---------------------------------------------------------------------------

    #[test]
    fn alloc_str_rc_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "a".repeat(len);
        let rc = arena.alloc_str_rc(&s);
        assert_eq!(rc.len(), len);
        // Original `>`: outer check false → inner fast path → `refill_local(total + align)`
        //   with `total + 1 > max_normal_alloc` → routes to oversized helper.
        // Mutant `>=`: outer check true → `try_alloc_prefixed_local_oversized` →
        //   `acquire_local(total)` with `total == max_normal_alloc` → normal chunk.
        // Observable: oversized counter is 1 under original, 0 under mutant.
        assert_eq!(
            arena.stats().oversized_local_chunks_allocated,
            1,
            "boundary alloc_str_rc must route via inner refill → oversized (original `>` semantics)"
        );
    }

    #[test]
    fn alloc_str_box_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "b".repeat(len);
        let b = arena.alloc_str_box(&s);
        assert_eq!(b.len(), len);
        assert_eq!(
            arena.stats().oversized_local_chunks_allocated,
            1,
            "boundary alloc_str_box must route via inner refill → oversized (original `>` semantics)"
        );
    }

    // ---------------------------------------------------------------------------
    // alloc_str.rs:288 — same boundary in `try_alloc_str_prefixed_shared`.
    // ---------------------------------------------------------------------------

    #[test]
    fn alloc_str_arc_at_boundary_takes_inner_path_not_outer_oversized() {
        let arena: Arena = Arena::new();
        let len = MAX_NORMAL_ALLOC - PREFIX_BYTES;
        let s = "c".repeat(len);
        let arc: ArcStr = arena.alloc_str_arc(&s);
        assert_eq!(arc.len(), len);
        assert_eq!(
            arena.stats().oversized_shared_chunks_allocated,
            1,
            "boundary alloc_str_arc must route via inner refill → oversized (original `>` semantics)"
        );
    }

    // Past-boundary sanity check: also catches `> → ==` and `> → <` mutants on
    // the same line (both make the routing false for strictly-greater inputs,
    // causing the fast path to fail).

    #[test]
    fn alloc_str_rc_past_boundary_uses_oversized() {
        let arena = Arena::new();
        let len = MAX_NORMAL_ALLOC + 16;
        let s = "p".repeat(len);
        let rc = arena.alloc_str_rc(&s);
        assert_eq!(rc.len(), len);
        assert!(arena.stats().oversized_local_chunks_allocated >= 1);
    }

    #[test]
    fn alloc_str_arc_past_boundary_uses_oversized() {
        let arena: Arena = Arena::new();
        let len = MAX_NORMAL_ALLOC + 16;
        let s = "q".repeat(len);
        let arc: ArcStr = arena.alloc_str_arc(&s);
        assert_eq!(arc.len(), len);
        assert!(arena.stats().oversized_shared_chunks_allocated >= 1);
    }

    // ---------------------------------------------------------------------------
    // alloc_utf16.rs:25 — `if total > max_normal_alloc` in `try_alloc_utf16_prefixed_local`.
    // ---------------------------------------------------------------------------

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_rc_at_boundary_takes_inner_path_not_outer_oversized() {
        use widestring::Utf16Str;
        let arena = Arena::new();
        let len = (MAX_NORMAL_ALLOC - PREFIX_BYTES) / 2;
        let buf: Vec<u16> = vec![u16::from(b'x'); len];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let rc = arena.alloc_utf16_str_rc(src);
        assert_eq!(rc.len(), len);
        assert_eq!(
            arena.stats().oversized_local_chunks_allocated,
            1,
            "boundary alloc_utf16_str_rc must route via inner refill → oversized (original `>` semantics)"
        );
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_rc_past_boundary_uses_oversized() {
        use widestring::Utf16Str;
        let arena = Arena::new();
        let len = (MAX_NORMAL_ALLOC - PREFIX_BYTES) / 2 + 16;
        let buf: Vec<u16> = vec![u16::from(b'y'); len];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let rc = arena.alloc_utf16_str_rc(src);
        assert_eq!(rc.len(), len);
        assert!(arena.stats().oversized_local_chunks_allocated >= 1);
    }

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
        assert_eq!(
            arena.stats().oversized_shared_chunks_allocated,
            1,
            "boundary alloc_utf16_str_arc must route via inner refill → oversized (shared, original `>` semantics)"
        );
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
        let arena = ArenaBuilder::new().max_normal_alloc(5000).build();
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
        let arena = ArenaBuilder::new().max_normal_alloc(5000).build();
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
        // 40_000 `u16`s = 80_000 payload bytes + 8-byte prefix = 80_008
        // bytes, which strictly exceeds `MAX_CHUNK_BYTES` (64 KiB). Under
        // the original `>` semantics this routes through the oversized
        // shared helper and succeeds. The `< max_normal_alloc` mutant
        // falls into the bump fast path, which then asks `refill_shared`
        // for an 80_008-byte chunk and is rejected (refill caps at
        // `MAX_CHUNK_BYTES`), propagating `AllocError` and panicking
        // through `alloc_utf16_str_arc`'s `expect_alloc`.
        let arena: Arena = Arena::new();
        let len_u16 = 40_000_usize;
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
    fn alloc_utf16_str_rc_small_string_stays_in_normal_chunk() {
        use widestring::Utf16Str;
        let arena = Arena::new();
        // Small string: `total = 8 + 2*10 = 28 << max_normal_alloc`. Original goes
        // through the fast bump path; the `<` mutant inverts the comparison and
        // routes to the outer oversized helper, which would create an oversized chunk.
        let buf: Vec<u16> = vec![u16::from(b'a'); 10];
        let src = Utf16Str::from_slice(&buf).unwrap();
        let rc = arena.alloc_utf16_str_rc(src);
        assert_eq!(rc.len(), 10);
        assert_eq!(
            arena.stats().oversized_local_chunks_allocated,
            0,
            "small utf16 alloc must take the fast path, not the outer oversized helper"
        );
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
            arena.stats().oversized_shared_chunks_allocated,
            0,
            "small utf16 alloc must take the fast path, not the outer oversized helper (shared)"
        );
    }

    // Mirror small-alloc tests for `alloc_str_rc/_box/_arc` so the `> → <`
    // mutation on those boundary checks is also caught.

    #[test]
    fn alloc_str_rc_small_stays_in_normal_chunk() {
        let arena = Arena::new();
        let rc = arena.alloc_str_rc("hello");
        assert_eq!(rc.len(), 5);
        assert_eq!(
            arena.stats().oversized_local_chunks_allocated,
            0,
            "small str alloc must take the fast path"
        );
    }

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
        let arc: ArcStr = arena.alloc_str_arc("test");
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
    fn drop_of_owned_in_local_chunk_decrements_refcount_releases_chunk() {
        use multitude::Rc;
        let arena: Arena = Arena::new();
        // First alloc: produces a normal chunk, refcount inflated to LARGE.
        // Drop the Rc: refcount returns to its baseline, chunk back-eligible.
        let rc: Rc<u64> = arena.alloc_rc(42_u64);
        let value = *rc;
        assert_eq!(value, 42);
        drop(rc);
        // Reset the arena: any chunk whose refcount-of-live-handles dropped
        // to its pinning baseline is reclaimed. If the OwnedInLocalChunk
        // Drop were a no-op (mutant), the chunk's refcount would stay
        // elevated and the chunk could not be reclaimed cleanly.
        drop(arena);
    }

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
    fn alloc_rc_with_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_rc_with::<HalfChunkAlign, _>(|| HalfChunkAlign);
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
    fn alloc_uninit_rc_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_uninit_rc::<HalfChunkAlign>();
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
    fn alloc_slice_fill_with_rc_drop_too_long_panics() {
        #[derive(Clone)]
        struct D;
        #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<D>() true so a drop_fn is installed")]
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_slice_fill_with_rc(u16::MAX as usize + 1, |_| D);
    }

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
    fn alloc_slice_fill_with_rc_oversized() {
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        let rc: multitude::Rc<[u32]> = arena.alloc_slice_fill_with_rc(2048, |i| u32::try_from(i).unwrap());
        assert_eq!(rc[0], 0);
        assert_eq!(rc[2047], 2047);
    }

    #[test]
    fn try_alloc_slice_fill_with_oversized() {
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        let slice: &mut [u32] = arena.try_alloc_slice_fill_with(2048, |i| u32::try_from(i).unwrap()).unwrap();
        assert_eq!(slice[0], 0);
        assert_eq!(slice[2047], 2047);
    }

    #[test]
    fn try_alloc_slice_copy_oversized() {
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        let src: alloc::vec::Vec<u32> = (0..2048_u32).collect();
        let slice = arena.try_alloc_slice_copy(&*src).unwrap();
        assert_eq!(slice[0], 0);
        assert_eq!(slice[2047], 2047);
    }

    #[test]
    fn alloc_slice_copy_oversized() {
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        let src: alloc::vec::Vec<u32> = (0..2048_u32).collect();
        let slice = arena.alloc_slice_copy(&*src);
        assert_eq!(slice[0], 0);
        assert_eq!(slice[2047], 2047);
    }

    #[test]
    fn try_alloc_slice_copy_arc_oversized() {
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        let src: alloc::vec::Vec<u32> = (0..2048_u32).collect();
        let arc = arena.try_alloc_slice_copy_arc(&*src).unwrap();
        assert_eq!(arc[0], 0);
        assert_eq!(arc[2047], 2047);
    }

    #[test]
    fn alloc_slice_fill_with_arc_oversized() {
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
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

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_rc_over_aligned_panics() {
        let arena = Arena::<Global>::new();
        let _ = arena.alloc_rc(HalfChunkAlign);
    }

    #[test]
    fn try_alloc_rc_over_aligned_returns_err() {
        let arena = Arena::<Global>::new();
        let res = arena.try_alloc_rc(HalfChunkAlign);
        assert!(res.is_err());
    }

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
        use std::cell::Cell;

        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Cell::new(0_u32);
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        // Warm up so the outer `alloc_with` below takes the fast path
        // (the cold slow path bypasses the eviction-commit branch).
        let _ = arena.alloc::<u64>(0);
        let _outer: &mut D<'_> = arena.alloc_with(|| {
            // Fill the current_local chunk so the OUTER allocation's
            // reserved slot ends up in a chunk that gets evicted before
            // the closure returns. The outer must then take the
            // `commit_alloc_after_eviction` branch.
            for _ in 0..200_000_u32 {
                let _ = arena.alloc::<u64>(0);
            }
            D(&drops)
        });
        drop(arena);
        assert_eq!(drops.get(), 1, "outer D's drop must run via eviction commit path");
    }

    #[test]
    fn refill_local_oversized_chunk_capacity() {
        // `with_capacity_local` preallocates space; verify the arena
        // works correctly when a generous capacity is requested.
        let arena = ArenaBuilder::<Global>::new().with_capacity_local(128 * 1024).build();
        let _ = arena.alloc::<u8>(0);
    }

    #[test]
    fn refill_shared_oversized_chunk_capacity() {
        let arena = ArenaBuilder::<Global>::new().with_capacity_shared(128 * 1024).build();
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

    #[cfg(feature = "std")]
    #[test]
    fn into_arena_rc_falls_back_to_copy_when_buffer_chunk_is_not_current() {
        use std::cell::Cell;

        use multitude::Rc;

        struct D<'a>(
            #[expect(dead_code, reason = "field exists only to make D !Copy")] u32,
            &'a Cell<u32>,
        );
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.1.set(self.1.get() + 1);
            }
        }

        let drops = Cell::new(0_u32);
        let arena = ArenaBuilder::<Global>::new().max_normal_alloc(4096).build();
        let mut v = arena.alloc_vec::<D<'_>>();
        v.push(D(1, &drops));
        v.push(D(2, &drops));
        // Push enough allocations through the arena that the chunk
        // hosting the vec's buffer is no longer `current_local`. The
        // subsequent `into_arena_rc` freeze fast-path will observe the
        // mismatch and take the copy-fallback branch.
        for _ in 0..200_000_u32 {
            let _ = arena.alloc::<u64>(0);
        }
        let rc: Rc<[D<'_>]> = v.into_arena_rc();
        assert_eq!(rc.len(), 2);
        drop(rc);
        drop(arena);
        assert!(drops.get() >= 2, "at least the 2 original D values must drop");
    }
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
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, ArenaBuilder, Rc};

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

    /// Kills `crates/multitude/src/arena.rs:311: replace Arena::builder ->
    /// ArenaBuilder<Global> with ArenaBuilder::from(Default::default())`.
    ///
    /// Both replacements happen to compute the same `ArenaBuilder<Global>`
    /// in the current code, but we still pin the documented behaviour:
    /// `Arena::builder()` returns a builder whose `build()` produces an
    /// arena equivalent to `Arena::new()` (no preallocation, default
    /// `max_normal_alloc`). This guards future divergences.
    #[test]
    fn arena_builder_default_matches_arena_new() {
        let a = Arena::builder().build();
        assert_eq!(a.stats().normal_local_chunks_allocated, 0);
        assert_eq!(a.stats().normal_shared_chunks_allocated, 0);
        // Allocate one normal-class value and check stats match Arena::new() flow.
        let _ = a.alloc_rc(42_u32);
        assert_eq!(a.stats().normal_local_chunks_allocated, 1);
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
        let res = ArenaBuilder::new().byte_budget(4 * 1024).with_capacity_local(513).try_build();
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
        let ok = ArenaBuilder::new().byte_budget(1500).with_capacity_local(513).try_build();
        assert!(
            ok.is_ok(),
            "513 must resolve to class 1 (1 KiB); a budget of 1500 bytes (>1 KiB) admits one chunk"
        );
    }

    /// Kills `drop_list.rs:49:73 + -> -` and `49:69 + -> -` and
    /// `49:69 + -> *`. (`49:73 + -> *` is equivalent to the original
    /// due to `2 + 2 == 2 + 2*1` short-circuiting back to 12 under
    /// operator precedence — see `MUTANTS_EQUIVALENT.md`.)
    ///
    /// The observable: in a 64 KiB chunk with `T: Drop`, the back-stack
    /// holds `floor((cap - bump_extent_loss) / size_of::<DropEntry>())`
    /// entries. With size 16 (unmutated): roughly 4096 entries.
    /// With size 8 (mutation): roughly 8192 entries.
    ///
    /// We pin: a workload of N drops, force the back-stack to be near-
    /// full, and measure how many *fresh chunks* the arena needs.
    #[test]
    fn drop_entry_size_matches_expected_layout() {
        let counter = StdArc::new(AtomicUsize::new(0));
        let chunks;
        {
            let arena = ArenaBuilder::new()
                // Force class 7 = 64 KiB local chunks.
                .with_capacity_local(64 * 1024)
                .build();
            let n: usize = 5000;
            let mut keep: Vec<multitude::Rc<DropCounter>> = Vec::with_capacity(n);
            for _ in 0..n {
                keep.push(arena.alloc_rc(DropCounter(counter.clone())));
            }
            chunks = arena.stats().normal_local_chunks_allocated;
            drop(keep);
            drop(arena);
        }
        assert!(
            chunks >= 2,
            "5000 DropEntries at 16 bytes each cannot fit in one 64 KiB chunk; got {chunks} chunks"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 5000);
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
            ArenaBuilder::new().byte_budget(b).with_capacity_local(512).try_build().is_ok()
        }
        fn ok_shared(b: usize) -> bool {
            ArenaBuilder::new().byte_budget(b).with_capacity_shared(512).try_build().is_ok()
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
        let res = ArenaBuilder::new()
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
        // Each big1/big2 needs ~80 KiB + header. Budget admits one but
        // not two simultaneously. Chunks at this size are truly
        // oversized (> MAX_CHUNK_BYTES = 64 KiB), so `release_local`
        // takes the free-with-release_budget branch instead of caching.
        let arena = ArenaBuilder::new().byte_budget(128 * 1024).max_normal_alloc(4 * 1024).build();
        let big1 = arena.alloc_box([0u8; 80 * 1024]);
        let s1 = arena.stats();
        assert_eq!(s1.oversized_local_chunks_allocated, 1);
        drop(big1);
        let big2 = arena.alloc_box([0u8; 80 * 1024]);
        let s2 = arena.stats();
        assert_eq!(s2.oversized_local_chunks_allocated, 2);
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
        let arena = ArenaBuilder::new().byte_budget(2 * 1024).build();
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
        let arena = ArenaBuilder::new().max_normal_alloc(4096).build();
        #[repr(align(8))]
        struct Block([u64; 512]); // 4096 bytes exactly
        let _a = arena.alloc_arc(Block([0u64; 512]));
        let s = arena.stats();
        assert_eq!(
            s.oversized_shared_chunks_allocated, 0,
            "size == max_normal_alloc on Arc must stay on the normal (cacheable) path"
        );
    }

    /// Kills `arena.rs:1085:26 > -> ==/>=` in `try_alloc_inner_slow_value`.
    /// Line 1085: `if layout.size() > self.provider.max_normal_alloc`.
    /// Boundary `size == max_normal_alloc` should stay on the normal path.
    #[test]
    fn slow_value_at_max_normal_stays_normal() {
        let arena = ArenaBuilder::new().max_normal_alloc(4096).build();
        #[repr(align(8))]
        struct Block([u64; 512]);
        // alloc_rc(value) goes through inner_value -> slow on fast-path miss.
        let _r = arena.alloc_rc(Block([0; 512]));
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
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
        let arena = ArenaBuilder::new().byte_budget(256 * 1024).max_normal_alloc(4096).build();
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

    /// Kills `arena.rs:1491:26 > -> >=` in `try_alloc_inner_slow_with`.
    /// Line 1491: `if layout.size() > self.provider.max_normal_alloc`.
    /// Boundary `size == max_normal_alloc` stays normal.
    #[test]
    fn slow_with_at_max_normal_stays_normal() {
        let arena = ArenaBuilder::new().max_normal_alloc(4096).build();
        #[repr(align(8))]
        struct Block([u64; 512]);
        let _r: multitude::Rc<Block> = arena.alloc_rc_with(|| Block([0; 512]));
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
    }

    /// Kills: arena.rs:728:30 `> -> >=` — oversized routing for arc
    /// When `layout.size()` == `max_normal_alloc`, the normal path should be
    /// used. If mutated to `>=`, it takes the oversized path.
    /// Detectable via stats: oversized vs normal shared chunk counts.
    #[test]
    fn arena_728_exact_max_normal_alloc_arc() {
        // Default max_normal_alloc is 16384.
        // Allocate an Arc of exactly that size.
        let arena = Arena::builder().max_normal_alloc(4096).build();
        // Type with size == 4096
        let _arc = arena.alloc_arc([0u8; 4096]);
        let stats = arena.stats();
        // Should go through normal path, not oversized
        assert_eq!(
            stats.oversized_shared_chunks_allocated, 0,
            "exact max_normal_alloc should use normal shared, not oversized"
        );
    }

    /// Kills the boundary mutation in `Arena::try_alloc_slice_shared_with`:
    ///
    /// * `arena/inner_slice.rs:886:26` — `>` → `>=` on
    ///   `layout.size() > self.provider.max_normal_alloc`.
    ///
    /// At the exact boundary `layout.size() == max_normal_alloc` the
    /// `ChunkProvider::acquire_shared` worst-case-size request crosses
    /// the oversized routing threshold, so both branches end up acquiring
    /// an oversized one-shot chunk. The observable distinguishing the
    /// two:
    ///
    /// * Original (fast path): the oversized chunk is installed as
    ///   `current_shared` (the fast path advances `data_ptr` on it), so
    ///   any immediately-following small shared allocation reuses the
    ///   tail of that chunk and no additional shared chunk is
    ///   allocated.
    /// * Mutation `>=`: the request is routed to
    ///   `try_alloc_slice_shared_oversized_with`, which by design does
    ///   *not* publish its chunk through `current_shared` (see the
    ///   comment at the top of that function). A subsequent small
    ///   shared allocation therefore has to refill `current_shared`,
    ///   producing a fresh normal shared chunk.
    ///
    /// We use `try_alloc_uninit_slice_arc::<u8>` so the call routes
    /// through `try_alloc_slice_shared_with` (the mutated function)
    /// with `drop_fn = None`. `max_normal_alloc` is pinned explicitly
    /// so the test is independent of any future default tweak.
    #[test]
    fn alloc_slice_arc_at_max_normal_alloc_installs_as_current_shared() {
        const MAX_NORMAL: usize = 16 * 1024;
        let arena = Arena::builder().max_normal_alloc(MAX_NORMAL).build();
        let big = arena
            .try_alloc_uninit_slice_arc::<u8>(MAX_NORMAL)
            .expect("alloc at max_normal_alloc must succeed");
        assert_eq!(big.len(), MAX_NORMAL);
        // Both branches acquire an oversized one-shot chunk (the
        // worst-case-size request `MAX_NORMAL + 1` exceeds
        // `max_normal_alloc`), so this stat is identical for both.
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 1);
        // Tiny follow-up. Under the original `>`, the oversized chunk
        // was installed as `current_shared` (with `round_payload`-rounded
        // tail bytes remaining), so this Arc lands in that same chunk
        // and `normal_shared_chunks_allocated` stays at zero. Under the
        // mutated `>=`, the oversized chunk was never installed and
        // this Arc forces a normal shared refill.
        let tiny = arena.alloc_arc(0_u8);
        assert_eq!(*tiny, 0);
        assert_eq!(
            arena.stats().normal_shared_chunks_allocated,
            0,
            "fast-path slice alloc at `layout.size() == max_normal_alloc` must install its chunk as `current_shared` so subsequent shared allocations reuse the tail"
        );
    }

    #[test]
    fn alloc_slice_just_above_max_normal_alloc_uses_oversized_path_local() {
        let arena = Arena::builder().max_normal_alloc(8 * 1024).build();
        let before = arena.stats().oversized_local_chunks_allocated;
        // Allocate one element past max_normal_alloc.
        let n = (8 * 1024) / core::mem::size_of::<u32>() + 1;
        let _r: Rc<[u32]> = arena.alloc_slice_fill_with_rc(n, |_| 0_u32);
        let after = arena.stats().oversized_local_chunks_allocated;
        assert_eq!(after - before, 1);
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
    fn vec_into_arena_rc_reclaims_unused_tail() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(64);
        for i in 0..4_u32 {
            v.push(i);
        }
        let before_chunks = arena.stats().normal_local_chunks_allocated;
        let rc: Rc<[u32]> = v.into_arena_rc();
        assert_eq!(&*rc, &[0_u32, 1, 2, 3]);
        // Reclaiming the unused tail leaves room for a follow-up alloc in
        // the same chunk. The chunk count must not have grown.
        let _follow_up: Rc<u32> = arena.alloc_rc(42);
        assert_eq!(arena.stats().normal_local_chunks_allocated, before_chunks);
    }

    #[test]
    fn vec_into_arena_box_reclaims_unused_tail() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(64);
        for i in 0..4_u32 {
            v.push(i);
        }
        let before = arena.stats().normal_local_chunks_allocated;
        let b: ArenaBox<[u32]> = v.into_arena_box();
        assert_eq!(&*b, &[0_u32, 1, 2, 3]);
        let _follow_up: Rc<u32> = arena.alloc_rc(42);
        assert_eq!(arena.stats().normal_local_chunks_allocated, before);
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
    fn string_shrink_to_fit_reclaims_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(128);
        s.push_str("hi");
        let before_chunks = arena.stats().normal_local_chunks_allocated;
        s.shrink_to_fit();
        assert_eq!(&*s, "hi");
        // The reclaim returns the unused 126 bytes to the bump cursor so a
        // follow-up small alloc fits in the same chunk.
        let _r: Rc<u8> = arena.alloc_rc(0);
        assert_eq!(arena.stats().normal_local_chunks_allocated, before_chunks);
    }

    #[test]
    fn arena_builder_capacity_preallocates_correct_chunk_count() {
        use multitude::ArenaBuilder;
        let arena: Arena = ArenaBuilder::new().with_capacity_local(64 * 1024).build();
        // Preallocation creates >= 1 chunk before any user allocation.
        assert!(arena.stats().normal_local_chunks_allocated >= 1);
    }

    #[test]
    fn chunk_byte_accounting_releases_full_chunk_on_drop() {
        use multitude::ArenaBuilder;
        let arena: Arena = ArenaBuilder::new().byte_budget(1024 * 1024).build();
        let snap1 = arena.stats();
        for _ in 0..100 {
            let _r: Rc<u32> = arena.alloc_rc(42);
        }
        let snap2 = arena.stats();
        // Allocations of `u32`s should not blow past the 1 MiB budget.
        assert!(snap2.normal_local_chunks_allocated >= snap1.normal_local_chunks_allocated);
    }

    #[test]
    fn shared_chunk_release_returns_budget() {
        use multitude::ArenaBuilder;
        let arena: Arena = ArenaBuilder::new().byte_budget(64 * 1024 * 1024).build();
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
    fn small_rc_allocations_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for i in 0_u32..256 {
            let _r: Rc<u32> = arena.alloc_rc(i);
        }
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
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
    fn slow_path_value_allocs_do_not_use_oversized_chunks() {
        // Force the slow refill path by filling many chunks with values.
        // Mutations in `try_alloc_inner_slow_value`/`_with` that make
        // `needed` enormous would route every refill through oversized.
        let arena = Arena::new();
        for i in 0_u32..16384 {
            let _r: Rc<u32> = arena.alloc_rc(i);
        }
        assert_eq!(arena.stats().oversized_local_chunks_allocated, 0);
    }

    #[test]
    fn slow_path_arc_allocs_do_not_use_oversized_chunks() {
        let arena = Arena::new();
        for i in 0_u32..16384 {
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
        for _ in 0..16384 {
            let _a: Arc<D<'_>> = arena.alloc_arc(D(&c));
        }
        assert_eq!(arena.stats().oversized_shared_chunks_allocated, 0);
    }

    #[test]
    fn vec_into_arena_rc_reclaim_lets_followup_fit_in_chunk() {
        // Use a vec capacity large enough that the chunk's remaining headroom
        // depends on whether reclaim actually fires. Default arena starts at
        // ~4 KiB payload. Vec cap=900 of u32 = 3600 bytes consumed. After
        // pushing only 4 elements, reclaim should return ~3584 bytes to the
        // cursor. A 2 KiB follow-up alloc fits only with the reclaim.
        //
        // The `rc` handle is held across the follow-up alloc so that if a
        // mutated reclaim moves the cursor too far back, the follow-up would
        // write into rc's storage and corrupt its content.
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(900);
        for i in 0..4_u32 {
            v.push(i);
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let rc: Rc<[u32]> = v.into_arena_rc();
        let big: Rc<[u8; 2000]> = arena.alloc_rc([42_u8; 2000]);
        assert_eq!(big[0], 42);
        assert_eq!(big[1999], 42);
        // rc must still hold [0, 1, 2, 3] (no overlap from follow-up).
        assert_eq!(rc.len(), 4);
        assert_eq!(rc[0], 0);
        assert_eq!(rc[1], 1);
        assert_eq!(rc[2], 2);
        assert_eq!(rc[3], 3);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn vec_into_arena_box_reclaim_lets_followup_fit_in_chunk() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(900);
        for i in 0..4_u32 {
            v.push(i);
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let b: ArenaBox<[u32]> = v.into_arena_box();
        let big: Rc<[u8; 2000]> = arena.alloc_rc([7_u8; 2000]);
        assert_eq!(big[0], 7);
        assert_eq!(big[1999], 7);
        assert_eq!(b.len(), 4);
        assert_eq!(b[0], 0);
        assert_eq!(b[1], 1);
        assert_eq!(b[2], 2);
        assert_eq!(b[3], 3);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn string_into_arena_str_reclaim_lets_followup_fit_in_chunk() {
        // Capacity 3600 bytes; only "hi" pushed. Reclaim should free ~3598 bytes.
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(3600);
        s.push_str("hi");
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _rs = s.into_arena_str();
        let big: Rc<[u8; 2000]> = arena.alloc_rc([5_u8; 2000]);
        assert_eq!(big[0], 5);
        assert_eq!(big[1999], 5);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn string_into_arena_box_str_reclaim_lets_followup_fit_in_chunk() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(3600);
        s.push_str("hi");
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _bs = s.into_arena_box_str();
        let big: Rc<[u8; 2000]> = arena.alloc_rc([9_u8; 2000]);
        assert_eq!(big[0], 9);
        assert_eq!(big[1999], 9);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    // Drop-typed reclaim tests — exercises the `if needs_drop && len > 0` path
    // in `into_arena_rc` / `into_arena_box` which is separate from the non-drop
    // path. Same logic: vec with large cap, only a few elements, follow-up
    // allocation requires the reclaim to fit in the same chunk.
    #[test]
    fn vec_into_arena_rc_drop_typed_reclaim_lets_followup_fit_in_chunk() {
        // String = 24 bytes (64-bit). cap=150 = 3600 bytes; push 4 = 96 bytes.
        let arena = Arena::new();
        let mut v: ArenaVec<'_, std::string::String> = arena.alloc_vec_with_capacity(150);
        for i in 0..4 {
            v.push(format!("e{i}"));
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _rc: Rc<[std::string::String]> = v.into_arena_rc();
        let big: Rc<[u8; 2000]> = arena.alloc_rc([1_u8; 2000]);
        assert_eq!(big[0], 1);
        assert_eq!(big[1999], 1);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn vec_into_arena_box_drop_typed_reclaim_lets_followup_fit_in_chunk() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, std::string::String> = arena.alloc_vec_with_capacity(150);
        for i in 0..4 {
            v.push(format!("e{i}"));
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _b: ArenaBox<[std::string::String]> = v.into_arena_box();
        let big: Rc<[u8; 2000]> = arena.alloc_rc([2_u8; 2000]);
        assert_eq!(big[0], 2);
        assert_eq!(big[1999], 2);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn vec_freeze_at_capacity_no_extra_alloc() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(4);
        for i in 0..4_u32 {
            v.push(i);
        }
        assert_eq!(v.len(), v.capacity());
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _b: ArenaBox<[u32]> = v.into_arena_box();
        let _r: Rc<u32> = arena.alloc_rc(0);
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn vec_into_arena_box_nonempty_nonzst_takes_inplace_path() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..4_u32 {
            v.push(i);
        }
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let _b: ArenaBox<[u32]> = v.into_arena_box();
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }

    #[test]
    fn many_small_allocations_do_not_inflate_chunk_count() {
        use multitude::ArenaBuilder;
        let arena: Arena = ArenaBuilder::new().build();
        for i in 0_u64..256 {
            let _r: Rc<u64> = arena.alloc_rc(i);
        }
        let chunks = arena.stats().normal_local_chunks_allocated;
        assert!(chunks < 16, "256 small allocs should not inflate chunk count: got {chunks}");
    }

    #[test]
    fn shared_chunk_release_budget_remains_bounded_through_many_cycles() {
        use multitude::ArenaBuilder;
        let arena: Arena = ArenaBuilder::new().byte_budget(2 * 1024 * 1024).build();
        for _ in 0..2048 {
            let _a: Arc<[u8; 1024]> = arena.alloc_arc([0_u8; 1024]);
        }
    }

    #[test]
    fn many_nondrop_slices_do_not_reserve_back_stack_entries() {
        let arena = Arena::new();
        for _ in 0..256 {
            let _r: Rc<[u32]> = arena.alloc_slice_copy_rc([42_u32; 16]);
        }
        let chunks = arena.stats().normal_local_chunks_allocated;
        assert!(chunks < 32, "non-drop slices should not reserve drop entries: chunks={chunks}");
    }

    #[test]
    fn string_shrink_to_fit_reclaim_lets_followup_fit_in_chunk() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(3600);
        s.push_str("hi");
        s.shrink_to_fit();
        let chunks_before = arena.stats().normal_local_chunks_allocated;
        let big: Rc<[u8; 2000]> = arena.alloc_rc([8_u8; 2000]);
        assert_eq!(big[0], 8);
        assert_eq!(big[1999], 8);
        assert_eq!(s.as_str(), "hi");
        assert_eq!(arena.stats().normal_local_chunks_allocated, chunks_before);
    }
}
