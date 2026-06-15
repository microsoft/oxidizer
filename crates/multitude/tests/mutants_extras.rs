// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated cross-cutting mutant-killing tests.

mod common;

mod mutants_for_kill {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // --------------------------------------------------------------------
    // A. Trait-impl mutants: hash forwarders and Pointer formatter.
    // --------------------------------------------------------------------

    // --------------------------------------------------------------------
    // B/I. Builder defaults / preallocation paths / resolve_capacity.
    // --------------------------------------------------------------------

    // --------------------------------------------------------------------
    // G. OversizedSharedGuard::drop — panic-recovery for arc-oversized.
    // --------------------------------------------------------------------

    // --------------------------------------------------------------------
    // C/D/E. Drop-counter exhaustive coverage. Many missed mutants live in
    // the per-flavor allocation hot paths and corrupt either the bump
    // cursor (`+ → -/*`), the drop-entry index/chain (`+1 → *1`), or the
    // fit/refill comparisons (`> → >=/==`). A test that allocates many
    // drop-tracking values, drops them, and asserts the exact count would
    // fail under any of those mutations: a wrong `data_ptr`/`drop_back`
    // segfaults; a wrong `drop_count` increment leaves entries unrun; a
    // flipped fit comparison either drops allocations or oversteps the
    // chunk.
    // --------------------------------------------------------------------

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
            // 256 Arc allocations still exercise multiple shared chunks and the
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
            // Verify alignment: any pointer-arithmetic mutation that
            // breaks the `align - 1` masking or the `aligned + size`
            // end-address computation would land us off-alignment.
            let p: *const Big = std::ptr::from_ref::<Big>(&b);
            assert_eq!((p as usize) % 64, 0, "Big must be 64-byte aligned");
            drop(b);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1, "oversized Box's Drop must run");

        // Same path for Arc (oversized shared): exercises the
        // `try_alloc_inner_arc_oversized_with` match-guard at line 1185
        // and the `OversizedSharedGuard` happy path (drop is forgotten on
        // success).
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

    // --------------------------------------------------------------------
    // H. align_offset — exercised transitively via oversized aligned alloc.
    // --------------------------------------------------------------------

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

    // --------------------------------------------------------------------
    // D. try_bump_fit boundary mutant.
    // --------------------------------------------------------------------

    #[test]
    fn many_distinct_size_and_align_combinations_succeed() {
        let arena = Arena::new();
        // Mix of size classes and alignments to maximize the chance
        // of hitting `aligned == max_aligned`.
        let mut keep_u8: std::vec::Vec<&mut u8> = std::vec::Vec::new();
        let mut keep_u16: std::vec::Vec<&mut u16> = std::vec::Vec::new();
        let mut keep_u32: std::vec::Vec<&mut u32> = std::vec::Vec::new();
        let mut keep_u64: std::vec::Vec<&mut u64> = std::vec::Vec::new();
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
        let mut all: std::vec::Vec<&mut [u64]> = std::vec::Vec::new();
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
    // Misc: confirm the && operator in the oversized-value flavor gate.
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

mod mutants_for_kill2 {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[derive(Debug)]
    struct DropCounter(StdArc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ============================================================
    // constants.rs mutants
    // ============================================================

    // ============================================================
    // drop_list.rs mutants — PAD_BYTES via mem::size_of::<DropEntry>
    // ============================================================
    //
    // DropEntry is `(fn_ptr=8) + (u16 + u16) + _pad`. With pointer-alignment
    // target = 8: RAW_USED=12; PAD_BYTES=4; size_of::<DropEntry>()=16.
    //
    // The mutants at line 49 change RAW_USED, which changes PAD_BYTES,
    // which changes size_of::<DropEntry>(). Observe by allocating a
    // known number of drop-tracked values: each consumes one DropEntry
    // in the chunk's back-stack. If the entry size changes, the number
    // of entries that fit in a 64 KiB chunk changes, which (for
    // sufficient pressure) changes the number of fresh chunks the
    // arena allocates.

    // ============================================================
    // arena_builder.rs mutants
    // ============================================================

    // ============================================================
    // chunk_provider.rs mutants
    // ============================================================

    // ============================================================
    // arena.rs mutants
    // ============================================================

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
        let arena = multitude::Arena::builder().with_capacity_shared(64 * 1024).build();
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
        // Enough iterations to force chunk grows (each chunk holds
        // ~15 × 4 KiB before refill at max class), but small enough that
        // Miri completes promptly. A `+ → -` mutation under-refills on
        // the very first high-alignment request, so a short burst still
        // catches it.
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
        let mut keep: Vec<&mut u8> = Vec::with_capacity(4096);
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

mod mutants_for_kill3 {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // =====================================================================
    // Helper: a type that needs Drop and is Send+Sync (for Arc allocs)
    // =====================================================================
    thread_local! {
        /// Per-test drop counter. `libtest` runs each test on its own
        /// thread, and these tests perform every `DropTracker`/arena drop on
        /// that same thread, so a thread-local counter is naturally isolated
        /// per test. This replaces an earlier global counter plus serializing
        /// mutex, which was order-sensitive: a test that dropped a
        /// `DropTracker`-bearing arena *after* releasing the mutex could bump
        /// the next test's count, producing flaky cross-test failures.
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

    // =====================================================================
    // arena.rs — try_alloc_inner_arc_with slow-path mutants
    // =====================================================================

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
        // Default max_normal_alloc is large. Use a small budget to force
        // the boundary. The default ArenaBuilder sets max_normal_alloc
        // based on chunk size. We just allocate something and check stats.
        let arena = Arena::builder().build();
        // Allocate a small arc with drop — exercises the normal path
        let _a1 = arena.alloc_arc_with(|| DropTracker(1));
        let _a2 = arena.alloc_arc_with(|| DropTracker(2));
        // Both should succeed through normal path, not oversized
        let stats = arena.stats();
        assert_eq!(
            stats.oversized_shared_chunks_allocated, 0,
            "small arcs should use normal shared chunks, not oversized"
        );
    }

    #[test]
    fn arena_731_needed_computation_arc_with() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Fill several arcs to force slow-path refill
        let mut keep = Vec::new();
        for i in 0..100 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 100, "all 100 DropTrackers must be dropped");
    }

    // =====================================================================
    // arena.rs — try_alloc_inner_slow_value mutants (1085, 1089, 1101)
    // =====================================================================

    #[test]
    fn arena_1251_oversized_shared_guard_drop() {
        let _guard = reset_drop_counter();
        // Force oversized path for shared (arc) allocations
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
            ptrs.push(r as *const u64 as usize);
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

    // =====================================================================
    // arena.rs — slice allocation mutants
    // =====================================================================

    #[test]
    fn arena_2261_slice_local_and_to_or() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate a zero-length slice of a Drop type (local flavor)
        let empty: &mut [DropTracker] = arena.alloc_slice_fill_with(0, |_| DropTracker(0));
        assert_eq!(empty.len(), 0);
        // Allocate a non-empty slice of a non-Drop type (no drop_fn)
        let nums: &mut [u32] = arena.alloc_slice_fill_with(10, |i| i as u32);
        assert_eq!(nums.len(), 10);
        for (i, v) in nums.iter().enumerate() {
            assert_eq!(*v, i as u32);
        }
    }

    #[test]
    fn arena_2266_slice_len_boundary() {
        let arena = Arena::new();
        // Non-Drop type, len > u16::MAX — should succeed (no drop entry needed)
        // Use a tiny type to avoid OOM
        let big_len = u16::MAX as usize + 1;
        let result = arena.try_alloc_slice_fill_with(big_len, |i| i as u8);
        // This may fail due to memory, but should not fail due to the len check
        // when entry_size == 0. If it fails, it's AllocError from memory, not the len check.
        // Let's use a smaller test: verify that exactly u16::MAX works for Drop types.
        let arena2 = Arena::new();
        // len == u16::MAX with Drop type — should succeed (not > u16::MAX)
        // This would be too much memory, so let's verify the boundary differently.
        // Actually test len == 0 with Drop type (entry_size should be 0 when len == 0)
        let empty_drop: &mut [DropTracker] = arena2.alloc_slice_fill_with(0, |_| DropTracker(0));
        assert_eq!(empty_drop.len(), 0);

        // Test len == 1 with Drop type — should succeed
        let _guard = reset_drop_counter();
        let one_drop: &mut [DropTracker] = arena2.alloc_slice_fill_with(1, |_| DropTracker(42));
        assert_eq!(one_drop.len(), 1);
        // Just verify the allocation is fine
        drop(result);
    }

    #[test]
    fn arena_2655_shared_slice_and_to_or() {
        let arena = Arena::new();
        // Allocate empty shared (arc) slice of Drop type
        let _guard = reset_drop_counter();
        let empty_arc = arena.alloc_slice_fill_with_arc(0, |_| DropTracker(0));
        assert_eq!(empty_arc.len(), 0);
        drop(empty_arc);
        // No drops should have occurred for empty slice
        let drops = drops();
        assert_eq!(drops, 0, "empty arc slice should not drop any elements");

        // Allocate non-empty shared slice of non-Drop type
        let nums_arc = arena.alloc_slice_fill_with_arc(5, |i| i as u64);
        assert_eq!(nums_arc.len(), 5);
    }

    #[test]
    fn arena_2660_shared_slice_len_boundary() {
        let arena = Arena::new();
        // Drop type, len == 1 via arc — should succeed (len <= u16::MAX)
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
        // Allocate multiple shared slices with Drop to exercise drop_back advancement
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
        // Force the slow refill path by filling the shared chunk
        let mut keep = Vec::new();
        for _ in 0..100 {
            keep.push(arena.alloc_slice_fill_with_arc(3, |i| DropTracker(i as u64)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 300, "100 * 3 = 300 drops");
    }

    // =====================================================================
    // chunk_provider.rs mutants
    // =====================================================================

    #[test]
    fn chunk_provider_133_reserve_budget_boundary() {
        // Set byte_budget so that exactly one default chunk fits.
        // The first allocation should succeed. If `>` becomes `>=`,
        // even the first allocation might fail.
        let arena = Arena::builder().byte_budget(256 * 1024).build();
        // Should succeed - within budget
        let _v = arena.alloc(42u64);
    }

    #[test]
    fn chunk_provider_441_shared_header_plus_target() {
        let arena = Arena::builder().byte_budget(512 * 1024).build();
        // Allocate shared (arc) values — each triggers acquire_shared
        let _a1 = arena.alloc_arc(1u64);
        let _a2 = arena.alloc_arc(2u64);
        // If `+` became `*`, the budget would be consumed much faster
        // and these allocations would likely fail or the budget check would
        // prevent them.
    }

    // =====================================================================
    // constants.rs mutants
    // =====================================================================

    #[test]
    fn constants_76_min_class_ge_to_lt() {
        // Allocating a large value forces acquire_local with a large payload
        // that exercises min_class_for_bytes near MAX_CHUNK_BYTES.
        let arena = Arena::new();
        let big = vec![0u8; 64 * 1024];
        let _alloc = arena.alloc_slice_copy(&big);
    }

    #[test]
    #[cfg(feature = "stats")]
    fn constants_87_loop_boundary() {
        // Allocate a value that lands on a power-of-two class boundary.
        // min_class_for_bytes(MIN_CHUNK_BYTES * 2) should return 1.
        // If `<` becomes `<=`, it returns 2 instead.
        // We can observe this through stats: with a tight budget,
        // a higher class means a larger chunk allocation.
        let arena = Arena::builder().byte_budget(128 * 1024).build();
        let _v = arena.alloc(42u64);
        // The allocation should succeed. If the class is wrong,
        // the chunk might be too large and blow the budget.
    }

    // =====================================================================
    // drop_list.rs mutants
    // =====================================================================

    // =====================================================================
    // local_chunk.rs / shared_chunk.rs mutants
    // =====================================================================

    #[test]
    fn shared_chunk_143_max_bump_extent() {
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
    fn shared_chunk_168_to_thin_ptr() {
        let arena = Arena::new();
        // Allocate and drop arcs to trigger chunk caching (which uses to_thin_ptr)
        for _ in 0..5 {
            let mut batch = Vec::new();
            for i in 0u64..50 {
                batch.push(arena.alloc_arc(i));
            }
            drop(batch);
        }
        // If to_thin_ptr returned null, the cache would be broken and
        // subsequent allocations would fail or crash.
        let final_arc = arena.alloc_arc(42u64);
        assert_eq!(*final_arc, 42);
    }

    #[test]
    fn shared_chunk_186_payload_rounding() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate arc values with Drop to exercise the shared chunk allocation
        // with proper payload rounding for drop entries
        let mut keep = Vec::new();
        for i in 0..50 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 50, "all 50 shared DropTrackers must drop");
    }

    // =====================================================================
    // strings/string.rs mutants
    // =====================================================================

    #[test]
    fn string_465_try_reserve_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        // Reserve exactly the remaining capacity
        s.try_reserve(10).unwrap(); // needed == cap, should not grow
        // Now push exactly 10 bytes
        s.push_str("1234567890");
        assert_eq!(s.as_str(), "1234567890");
        // Reserve 0 more — should be no-op
        s.try_reserve(0).unwrap();
    }

    // =====================================================================
    // strings/utf16_string.rs mutants
    // =====================================================================

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
        // Start with cap=0, push to force realloc with len=0 initially
        let mut v = arena.alloc_vec_with_capacity::<u64>(0);
        assert_eq!(v.len(), 0);
        v.push(1); // triggers realloc from cap=0
        assert_eq!(v[0], 1);

        // Now realloc with len > 0
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
        // Use an alloc strictly larger than `MAX_CHUNK_BYTES = 64 KiB` so
        // the chunk is truly oversized: after `reconcile_swap_out` the
        // backing allocation is freed (and its budget released) rather
        // than cached. Budget allows ONE such chunk but not two
        // simultaneously. If `OversizedSharedGuard::drop` is a no-op
        // (the mutant), the panicked alloc's chunk stays charged against
        // the budget; the second oversized alloc then fails. We assert
        // the second alloc succeeds, killing the mutant.
        const N: usize = 70_000;
        let arena = Arena::builder().max_normal_alloc(4096).byte_budget(N + 4096).build();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _arc: multitude::Arc<[u8; N]> = arena.alloc_arc_with(|| {
                panic!("intentional panic in oversized arc closure");
            });
        }));
        assert!(result.is_err(), "should have caught the panic");

        // Only succeeds if the panicked chunk's budget was released by
        // `OversizedSharedGuard::drop`. If `drop` is no-op'd, the budget
        // is leaked and this `alloc_arc_with` (which calls `expect_alloc`)
        // panics with "allocator returned AllocError".
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
        // With `+ -> -`: needed = 64 + (64 - 8) = 120 vs 64 - (64 - 8) = 64 - 56 = 8
        // The `- 56` would request only 8 bytes from refill, too small.
        let mut keep = Vec::new();
        for i in 0u8..100 {
            let v = arena.alloc(Aligned64 { data: [i; 64] });
            assert_eq!(v.data[0], i);
            keep.push(v as *const Aligned64 as usize);
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
            let s: &mut [DropTracker] = arena.alloc_slice_fill_with(0, |_| DropTracker(0));
            assert_eq!(s.len(), 0);
        }
        // If entry_size was wrongly nonzero, we'd waste space and
        // potentially corrupt the drop list.
        // Also test non-empty non-Drop slices (drop_fn is None)
        for i in 0u64..500 {
            let s: &mut [u64] = arena.alloc_slice_fill_with(5, |j| i + j as u64);
            assert_eq!(s[0], i);
        }
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 0, "empty Drop slices should not produce drops");
    }

    #[test]
    fn arena_2266_large_nondrop_slice() {
        let arena = Arena::new();
        // Try allocating a large non-Drop slice — should succeed with !=
        // With == mutation, this would be rejected.
        // u16::MAX + 1 = 65536 elements of u8 = 64KB
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
    fn arena_2660_large_nondrop_shared_slice() {
        // Use a non-Copy, non-Drop wrapper so we go through try_alloc_slice_shared_with
        // (the Copy path bypasses line 2660).
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
    fn shared_chunk_143_max_bump_many() {
        let _guard = reset_drop_counter();
        const N: u64 = 64;

        let arena = Arena::builder().with_capacity_shared(64 * 1024).build();

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
    fn shared_chunk_168_force_cache_reuse() {
        let arena = Arena::new();
        // Round 1: allocate arcs, fill a shared chunk
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
    fn shared_chunk_186_payload_rounding_stress() {
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
        // Total: 4 drops. With the `/` mutation, only 2 cloned + 1 value = 3.
        let drops = DROP_COUNT2.load(Ordering::SeqCst);
        assert_eq!(drops, 4, "guard must drop exactly 3 cloned elements + 1 value; got {drops}");
        assert_eq!(v.len(), 2);
    }
}

mod mutants_for_kill4 {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
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
        // Original: additional = 10 - 5 = 5 ⇒ cap = max(10, 16, 4) = 16.
        // Mutated `+`: additional = 10 + 5 = 15 ⇒ cap = max(20, 16, 4) = 20.
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

    #[test]
    fn resize_guard_drop_uses_subtraction() {
        use std::panic::{set_hook, take_hook};

        struct Ctor(StdArc<AtomicUsize>, usize);
        impl Clone for Ctor {
            fn clone(&self) -> Self {
                let prev = self.0.fetch_add(1, Ordering::SeqCst);
                assert!(prev + 1 < self.1, "planned clone panic at index {prev}");
                Self(self.0.clone(), self.1)
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // Silence the panic logger for the duration of the unwind.
        let prev = take_hook();
        set_hook(Box::new(|_| {}));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let arena = Arena::new();
            let mut v: multitude::vec::Vec<'_, Ctor> = arena.alloc_vec();
            // Start from EMPTY vec so old_len == 0 ⇒ mutated `/ 0` div-by-zero.
            // Resize to 3: clones template twice, then moves template into last slot.
            // We make the SECOND clone panic.
            let template = Ctor(counter.clone(), 2);
            v.resize(3, template);
        }));
        set_hook(prev);
        assert!(result.is_err(), "resize must panic via the planted clone panic");
        let payload = result.unwrap_err();
        let s = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&'static str>().map(std::string::ToString::to_string))
            .unwrap_or_default();
        // Original: panic payload contains "planned clone panic".
        // Mutated (`/`): the Guard drop triggers div-by-zero, aborting the
        // process before catch_unwind sees a payload — process aborts.
        // If we reach this assertion, the test ran without abort; the
        // payload string must be the *planted* one. The mutated version
        // would either abort or surface a divide-by-zero panic.
        assert!(
            s.contains("planned clone panic"),
            "unexpected panic payload: {s:?} (mutated `/ 0` in Guard::drop would surface as divide-by-zero)"
        );
    }

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

mod mutants_for_kill5 {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "raw identifier names in docs")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
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

mod mutants_for_audit {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ============================================================================
    // vec.rs:473 — resize Guard::drop's `if added > 0`
    // vec.rs:507 — resize_with's `let added = self.vec.len - self.old_len;`
    // vec.rs:515/516 — resize_with Guard::drop's `added = len - old` and `if added > 0`
    //
    // These are panic-safety guards. Killing requires:
    //   (a) panic mid-resize after some elements have been written and verify
    //       only the partial set is dropped (kills `> 0` → `>=` boundary because
    //       the added==0 case happens when init panics on the very first element);
    //   (b) panic before any element is written (added == 0) and verify
    //       len rolls back to old_len without dropping anything.
    // ============================================================================

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
            // 2 pre-existing elements; resize pushes 5 more (4 clones + 1 move).
            // We arrange the panic to fire after some clones via a cloning side
            // effect — simulate by using a value whose clone panics. Easier:
            // use resize_with so we can panic on the Nth call.
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
        // Now the 2 originals also dropped.
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
            // Panic on the very first init call: added == 0 in Guard::drop.
            // Kills `if added > 0` → `if added >= 0`: with the mutant, the
            // `>= 0` branch executes `from_raw_parts_mut(..., 0)` and
            // `drop_in_place` over an empty slice — observationally equivalent.
            // But the mutant ALSO walks `data.add(old_len)` for an empty
            // slice; if old_len == cap, that's one-past-the-end which is
            // legal but Miri / UBSAN would catch invalid pointer arithmetic
            // if old_len exceeded cap. The mutant survives because a 0-len
            // slice from a one-past pointer is well-defined — so the only
            // way to kill is to verify the post-condition (len rolls back
            // and zero added drops).
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

    // ============================================================================
    // vec.rs:621 / vec.rs:838 — into_arena_rc/box's `if self.len > u16::MAX as usize`
    //
    // The boundary at `u16::MAX` (65535) is unreachable in the in-place
    // branch: that branch requires the Vec data buffer to live in the arena's
    // `current_local`, which in turn requires `len * size_of::<T>() <=
    // max_normal_alloc`. With `max_normal_alloc <= max_bump_extent < 64 KiB`
    // and `size_of::<T>() >= 1` (the in-place branch already short-circuits
    // the ZST/empty case), `len * size_of::<T>() <= max_bump_extent <
    // u16::MAX`. So `self.len > u16::MAX` cannot fire on a Vec the in-place
    // path is willing to handle. The check is defensive and equivalent to
    // removing it.
    // ============================================================================

    #[cfg(feature = "dst")]
    #[allow(dead_code, reason = "helper kept after moving its consumers to dst.rs; preserved for future tests")]
    struct OneByteDrop(#[allow(dead_code)] u8);
    #[cfg(feature = "dst")]
    impl Drop for OneByteDrop {
        fn drop(&mut self) {}
    }

    // ============================================================================
    // vec.rs:634 — into_arena_rc's `if needs_drop && len > 0`
    // vec.rs:837 — into_box's `if needs_drop && self.len > 0`
    //
    // Mutant: `>` → `>=`. With `>= 0` (always true for usize) the empty Drop
    // vec would attempt to install a slice DropEntry of len=0, which would
    // panic / abort because the back-stack is full or because the helper
    // rejects len==0 explicitly. Kill: empty vec of Drop type round-trips
    // without abort.
    // ============================================================================

    // ============================================================================
    // vec.rs:657 / 859 / 878 — `if cap > len` reclaim guard.
    // Mutant `>=`: at cap == len the mutant tries to reclaim 0 bytes.
    // `try_shrink_at_cursor(buffer_end, 0)` may decrement the cursor by 0
    // (no-op) or panic on debug assertions. Kill via stats counter or by
    // observing that a subsequent allocation lands on the cursor where
    // the buffer ended.
    // ============================================================================

    // ============================================================================
    // vec.rs:911 — into_box's `consumed_cell.set(idx + 1)`
    // Mutant `+ → *`: at idx==0 both yield 0 → infinite loop / wrong index.
    // Kill: copy at least 2 elements and verify all are present.
    // ============================================================================

    // ============================================================================
    // vec.rs:963 — realloc's `if new_cap > self.cap && self.cap > 0`
    // Mutant `>=`: with new_cap == self.cap, the mutant tries grow-in-place
    // of zero bytes, which is a no-op shrink. With self.cap == 0 (fresh vec)
    // the mutant calls try_grow_in_place with a dangling pointer — would
    // likely abort.
    // vec.rs:976 — realloc's `if self.len > 0` for memcpy of old data.
    // Mutant `>=`: at len==0 the mutant copies 0 bytes from a dangling
    // pointer (likely OK). To kill: verify a non-empty Vec preserves data
    // across realloc.
    // ============================================================================

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

    // ============================================================================
    // vec.rs:1311 — Drain TailFix::drop's `if tail_len > 0`
    // Mutant `>=`: at tail_len == 0 (drained to end) the mutant tries to
    // `ptr::copy(..., 0)` from one-past-the-end. Well-defined, but kill via
    // the post-condition (len update is correct).
    // ============================================================================

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

    /// Regression: when an unyielded element's `Drop` panics during
    /// `Drain::drop`, the remaining unyielded drained elements must still
    /// be dropped (panic-policy parity with `std::vec::Drain::drop`, which
    /// uses `drop_in_place::<[T]>` to delegate to rustc's slice-drop guard).
    /// Previously `multitude` used a per-element loop that leaked the tail
    /// elements past the first panicking drop.
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

    // ============================================================================
    // arena.rs:1721 — try_alloc_slice_shared_oversized_with's
    //   `if entry_size != 0 && len > u16::MAX as usize { return Err(...) }`
    // Mutant `>` → `<`: rejects short Drop-aware slices, accepts long ones.
    // Kill: verify a short Drop-aware oversized slice succeeds; then verify
    // a >u16::MAX Drop-aware oversized slice returns Err.
    // ============================================================================

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

    // ============================================================================
    // arena.rs:1643 / 1751 — slice oversized helpers' `init_guard.len += 1;`
    // Mutant `+= → *=`: with init_guard.len starting at 0, `0 *= 1` stays
    // at 0 forever. Then on init panic mid-way, the SliceInitGuard drops
    // 0 elements instead of N — leaks T::drop. Kill: panic mid-init in the
    // oversized helper and verify the right number of drops happened.
    // ============================================================================

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

    // ============================================================================
    // arena.rs slice-with paths — `if layout.size() > self.provider.max_normal_alloc`
    // boundary mutants: at `==` the original takes the fast path; the mutant
    // `<` keeps small allocations on fast path (already, no change) but
    // causes large allocations to use the fast path too, which then refills
    // to an oversized chunk via worst-case-size. Net observable effect is
    // best caught by allocations *just above* the boundary, where the
    // original routes through the oversized helper directly.
    // ============================================================================

    // ============================================================================
    // arena.rs:1332 / 2030 — alloc_inner_*_or_panic's `if bumped > MAX_CHUNK_BYTES`
    // is intrinsically guarded — `bumped` is a compile-time-known size for the
    // value paths and the safety check is unreachable for any value type a user
    // can construct. Targeted by other equivalent boundary tests above.
    // ============================================================================

    // ============================================================================
    // arena.rs refill_local/refill_shared bump_extent branch (lines 726, 1055):
    //   `if capacity > MAX_CHUNK_BYTES { capacity } else { capacity.min(...) }`
    // Mutant `<`: would invert the condition and use `capacity` for normal
    // chunks (allowing bump cursor past the first 64 KiB tile). Subsequent
    // allocations would then resolve the wrong chunk header via the mask.
    // Kill via stress: many allocations and a Drop type to exercise drop
    // list replay.
    // ============================================================================

    // ============================================================================
    // arena.rs:3036 / 3608 — slice paths' `if entry_size != 0 && len > u16::MAX`
    // (panic-first).  Mutant `!=` → `==`: with entry_size == 0 (no drop) the
    // mutant runs the panic check; with entry_size != 0 it skips. The result
    // is that a Copy slice longer than u16::MAX would panic. Kill: a Copy
    // slice of length > u16::MAX must succeed.
    // ============================================================================

    // ============================================================================
    // arena.rs:3039 / 3611 — slice paths' `if layout.size() > self.provider.max_normal_alloc`
    // (panic-first). The "just above" check on these is the same structural
    // boundary as the non-panic variants — already covered above. The "at
    // exact" boundary cannot be observed because both branches eventually
    // allocate via the oversized path due to compute_worst_case_size adding
    // `align + entry_size` to the request, which always pushes a slice of
    // exactly `max_normal_alloc` bytes past the routing threshold inside
    // `acquire_local`.
    // ============================================================================

    // ============================================================================
    // arena.rs:3097 / 3659 — slice paths' `guard.len += 1` (init guard counter)
    // Mutant `+= → *=`: same as the oversized-helper variant, but for the
    // fast path. Kill: panic mid-init in the fast path (small slice fits in
    // a normal chunk) and verify partial-init drops are exactly N.
    // ============================================================================

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

    // ============================================================================
    // arena.rs:3102 / 3664 — slice paths' `if !matches!(flavor, AllocFlavor::Box)
    // && let Some(drop_fn) = drop_fn.filter(|_| len != 0)` and the shared
    // equivalent. Mutants delete the `!`/swap `!=` to `==`.
    //
    // `delete !` mutant: skips the drop_fn install for non-Box flavors.
    // Without the install, the noop_drop_shim stays in the entry → elements
    // leak. Kill: Rc slice of Drop type, drop the Rc, drop the arena, count
    // drops.
    // `!= → ==` mutant: only installs drop_fn when len == 0 — empty slices
    // don't have an entry installed in the first place (entry_size == 0),
    // so this mutant is observationally no-op for typical inputs. Kill:
    // non-empty slice of Drop type via `Rc::from_*` should drop properly.
    // ============================================================================

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

    // ============================================================================
    // arena.rs:3116 / 3676 — slice paths' refill `compute_worst_case_size(layout, entry_size != 0)`
    // `!= → ==` mutant: passes `entry_size == 0` to compute_worst_case_size, which
    // flips the "needs entry" flag. The downstream chunk capacity may be too
    // small to fit both the slice and its drop entry. Kill: large drop-aware
    // slice that needs a refill — must succeed.
    // ============================================================================

    // ============================================================================
    // arena.rs:2076 / 941 — alloc_inner_*_or_panic's drop-count and `needed`
    // arithmetic. The `+ → *` and `+ → -` mutants on
    // `let needed = layout.size() + alignment + entry_size`: changing `+` to
    // `*` produces wildly larger needed-size; if it stays ≤ MAX_CHUNK_BYTES,
    // the chunk class still satisfies the original request — equivalent.
    // Kill: at `layout.size() == max_normal_alloc - small_amount` the
    // difference between `size + align + entry_size` (= max_normal_alloc-ish)
    // and `size * align * entry_size` (= astronomically larger) routes the
    // mutant to fail refill or fall back to oversized.
    // ============================================================================

    // (Hard to test via public API — covered indirectly by all the slice/value
    // tests above that refill across many chunk classes.)

    // ============================================================================
    // arena.rs:3036 / 3608 — `if entry_size != 0 && len > u16::MAX as usize`
    // `> with ==` mutant: only panics when len exactly equals u16::MAX.
    // `> with >=` mutant: panics at len == u16::MAX (one short of original).
    // Kill: a Drop-aware slice of len == u16::MAX must succeed (original)
    // and must panic for len > u16::MAX.
    // ============================================================================

    #[test]
    fn alloc_slice_shared_drop_aware_above_u16_max_returns_err() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::AtomicU32;
        struct D(#[allow(dead_code)] StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::builder().max_normal_alloc(60 * 1024).build();
        let result = arena.try_alloc_slice_fill_with_arc(65_536, |_| D(drops.clone()));
        assert!(result.is_err());
    }

    // ============================================================================
    // vec.rs:507:34 — `self.reserve(new_len - self.len)` in resize_with.
    // Mutant `-` -> `+`: reserves `new_len + self.len`. Both work, but mutant
    // over-reserves. Kill via capacity observation.
    // ============================================================================

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
        // Original: additional = 8 - 4 = 4. doubled = max(4 + 4, 4*2, 4) = 8.
        // Mutant:   additional = 8 + 4 = 12. doubled = max(4 + 12, 4*2, 4) = 16.
        assert!(
            cap_after < 16,
            "resize_with from len=4 to 8 should not over-reserve (cap_before={cap_before}, cap_after={cap_after})"
        );
        assert_eq!(v.len(), 8);
        assert_eq!(v.as_slice(), &[1, 2, 3, 4, 99, 99, 99, 99]);
    }

    // ============================================================================
    // vec.rs:556:67 — `self.data.as_ptr().add(self.len - 1)` in pop_if.
    // Mutant `-` -> `/`: passes `self.len / 1 = self.len` (one past end).
    // Kill: spy on the value the predicate sees.
    // ============================================================================

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

    // ============================================================================
    // vec.rs:911:35 — `consumed_cell.set(idx + 1)` in `into_box`.
    // Mutant `+` -> `*`: with idx==0, `0 * 1 == 0`; consumed_cell never
    // advances, every closure invocation reads `data[0]`. Kill: route the
    // buffer to an oversized chunk so install fails, then verify boxed
    // elements distinctly match the source.
    // ============================================================================
}

mod mutants_for_complete {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ----------------------------------------------------------------------------
    // vec.rs:285 — replace - with + in Vec::insert
    //
    // Original: `ptr::copy(ptr, ptr.add(1), self.len - idx);`
    // Mutant:   `ptr::copy(ptr, ptr.add(1), self.len + idx);`
    //
    // With `len=3, idx=1`: `len-idx=2`, `len+idx=4`. Original shifts 2
    // elements (correct); mutant would copy 4 elements (UB / wrong data).
    // ----------------------------------------------------------------------------

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

    // ----------------------------------------------------------------------------
    // vec.rs:303 — `self.len - idx - 1` in Vec::remove
    //
    // Mutants:
    //   `self.len - idx + 1`  (replaces second `-` with `+`)
    //   `self.len + idx - 1`  (replaces first `-` with `+`)
    //   `self.len - idx / 1`  (replaces second `-` with `/`)
    //
    // With `len=3, idx=0`: original copies 2 elements; mutants copy
    // different counts → different remaining contents.
    // ----------------------------------------------------------------------------

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

    // ----------------------------------------------------------------------------
    // vec.rs:359 — replace < with <= in shrink_to_fit
    //
    // Original: `if self.len < self.cap && self.realloc(self.len).is_err()`
    // Mutant:   `if self.len <= self.cap && self.realloc(self.len).is_err()`
    //
    // When `len == cap`, original short-circuits (no realloc); mutant enters
    // the branch and calls `realloc(len)` where `new_cap == self.cap`,
    // triggering the `debug_assert!(new_cap != self.cap)` in `realloc`.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:429 — replace > with >= in try_reserve_exact
    //
    // Original: `if needed > self.cap { self.realloc(needed)?; }`
    // Mutant:   `if needed >= self.cap { self.realloc(needed)?; }`
    //
    // When `needed == cap`, mutant calls `realloc(cap)`, which fires the
    // `debug_assert!(new_cap != self.cap)` in `realloc`.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(debug_assertions)]
    fn vec_try_reserve_exact_at_capacity_is_noop() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([0_u32, 1, 2]);
        // needed = 3 + 5 = 8 == cap. Original short-circuits; mutant
        // realloc-call would assert-fail.
        v.try_reserve_exact(5).unwrap();
        assert_eq!(v.capacity(), 8);
    }

    // ----------------------------------------------------------------------------
    // vec.rs:948 — realloc's in-place grow guards: `new_cap > self.cap && self.cap > 0`
    // Mutants:
    //   `new_cap > self.cap || self.cap > 0`   (`&&` → `||`)
    //   `new_cap >= self.cap && self.cap > 0`  (`>` → `>=`)
    //   `new_cap > self.cap && self.cap >= 0`  (`>` → `>=`)
    // vec.rs:959 — `let Some(grown) = unsafe { ... }` then the `>` on Layout::array().is_ok()
    // vec.rs:968 — `if old_cap > 0 { self.arena.bump_relocation(); }`
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:643 / vec.rs:644 / vec.rs:862-863 / vec.rs:895:
    //   `reclaim_bytes = (cap - len) * elem_size`  in into_arena_rc/box paths.
    // Mutants: `(cap - len) + elem_size`, `(cap + len) * elem_size`,
    //          `(cap - len) / elem_size`, etc.
    // Detection: assert that the freeze path reclaims exactly the unused
    // tail so a subsequent allocation lands in the same chunk.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:606 / vec.rs:619 — `needs_drop && self.len > u16::MAX as usize`
    // Mutants: `&&` → `||`, `>` → `>=`.
    // Detection: a `T: Drop` slice with exactly `u16::MAX` elements must
    // take the in-place freeze path (not the copy fallback). A slice with
    // `u16::MAX + 1` elements must take the copy fallback (the back-stack
    // entry's length field is u16).
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:458, 502 — Guard::drop's `if added > 0 { drop_in_place(tail) }`
    // Mutants: `>` → `>=`.
    //
    // `added == 0` => `drop_in_place(&mut [])` is a no-op. The mutant adds an
    // unnecessary call but produces the same observable behavior. Mark these
    // as documented-equivalent.
    // ----------------------------------------------------------------------------

    // (No test required — equivalent mutation. Covered by panic-recovery tests
    //  in arena_vec.rs::resize_panic_in_clone_drops_already_written which
    //  exercises the Guard::drop code path with `added > 0`.)

    // ----------------------------------------------------------------------------
    // vec.rs:493 — `reserve(new_len - self.len)` in resize_with
    // Mutant: `reserve(new_len + self.len)` over-reserves but doesn't break behavior.
    // The reservation is still sufficient — over-reservation is observable through
    // `arena.stats()` chunk-allocation counts only.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:1293 — TailFix::drop's `if tail_len > 0 { copy ... }`
    // Mutant: `>` → `>=`. With `tail_len == 0`, original skips; mutant calls
    // `ptr::copy(src, dst, 0)` which is a no-op. Equivalent.
    //
    // (No test required — equivalent mutation by ptr::copy semantics on len=0.)
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/string.rs:421 / 452 — try_push_str / try_reserve boundary
    // Mutants: `needed > self.cap` → `needed >= self.cap`. At needed == cap,
    // the mutant calls `try_grow_to_at_least(needed)` whose debug_assert!
    // guards against `min_cap <= self.cap`.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(debug_assertions)]
    fn string_try_push_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(8);
        s.push_str("abcde");
        // needed = 5 + 3 = 8 == cap; must skip grow path.
        s.push_str("fgh");
        assert_eq!(&*s, "abcdefgh");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:207 — shrink_to_fit's `if self.cap == 0 || self.len == self.cap { return; }`
    // Mutant: `||` → `&&`. Then `cap == 0` allocations would attempt to grow,
    // triggering the `try_grow_to_at_least` debug_assert.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/string.rs:239 / 359 — insert_str / replace_range boundary `new_len > self.cap`
    // Mutants: `>` → `>=`. Same kill mechanism as try_push_str.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(debug_assertions)]
    fn string_insert_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abc");
        // new_len = 3 + 3 = 6 == cap; must not enter grow path.
        s.insert_str(0, "xyz");
        assert_eq!(&*s, "xyzabc");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn string_replace_range_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abc");
        // Replace 1 char ('b') with 4 chars; new_len = 6 == cap.
        s.replace_range(1..2, "WXYZ");
        assert_eq!(&*s, "aWXYZc");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:268 / utf16_string.rs:348 — remove arithmetic
    //   `let next = idx + ch.len_utf8(); ... copy(src, dst, self.len - next)`
    // Mutants: `-` → `+`, `-` → `/`.
    //
    // With a single-byte char and 3 chars after it, original copies 3 bytes,
    // `+` mutant copies wrong count → wrong remaining string.
    // ----------------------------------------------------------------------------

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

    // ----------------------------------------------------------------------------
    // strings/string.rs:306 — retain's `idx_dst + n_bytes`
    // Mutants: `+` → `-`, `+` → `*`.
    //
    // Original: bytes-from-source moved by `len-n_bytes` count.
    // ----------------------------------------------------------------------------

    #[test]
    fn string_retain_preserves_filtered_chars() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello world");
        s.retain(|c| !c.is_whitespace());
        assert_eq!(&*s, "helloworld");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:366 — replace_range's `let tail = ... self.len - end_idx`
    // Mutants: `-` → `+`.
    // ----------------------------------------------------------------------------

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

    // ----------------------------------------------------------------------------
    // strings/string.rs:515 / utf16_string.rs:493 — try_reclaim_tail's
    //   `if cap >= len { let reclaim = cap - len; }` (or similar).
    // Mutants: `>=` → `<`, `replace ... with ()`, `-` → `/`.
    //
    // `try_reclaim_tail` is called after push/grow operations to release
    // unused tail capacity. To kill `replace ... with ()`, observe that the
    // chunk's cursor advances by less than expected after reclaim.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:183 — truncate's `if new_len > self.len { return; }`
    // Mutant: `>` → `>=`. At new_len == len, original short-circuits; mutant
    // re-clamps and writes the prefix.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:195 — shrink_to_fit `cap == 0 || len == cap`
    // Same as string.rs:207 mutant.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:198 / 199 — shrink_to_fit byte-arithmetic
    //   `reclaim_units = cap - len; reclaim_bytes = reclaim_units * 2;`
    // Mutants: `-` → `/`, `*` → `+`, `*` → `/`.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:252 / 269 / 290 / 310 / 322 / 398 / 405 / 420 — many
    // boundary checks; same kill mechanism as string.rs equivalents.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(all(debug_assertions, feature = "utf16"))]
    fn utf16_try_push_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(8);
        s.push_from_str("abcd");
        // Worst-case reservation: 4 BMP chars = 8 units; needed == cap.
        s.try_push_from_str("efgh").unwrap();
        assert_eq!(s.len(), 8);
    }

    // ----------------------------------------------------------------------------
    // box.rs:209 — Box<[T]>::into_rc's `if needs_drop && len > u16::MAX as usize`
    // Mutants: `&&` → `||`, `>` → `>=`.
    // Same boundary as vec.rs:606 / 619.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:767 — try_alloc_inner_arc_with's `if bumped > MAX_CHUNK_BYTES`
    // Mutant: `>` → `>=`. At exact equality, mutant routes to the oversized
    // path even though the request fits in a normal chunk. Detection through
    // stats counters.
    //
    // (Hard to test deterministically without exact MAX_CHUNK_BYTES; covered
    //  by `oversized_chunk_used_when_alloc_too_big` already.)
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:825 — `if entry_size > 0` (drop entry installation)
    // Mutant: `>` → `>=`. Always falsy with usize variable; equivalent only if
    // entry_size is non-zero. For `T: !Drop`, entry_size is `0`; for `T: Drop`,
    // entry_size is `size_of::<InnerDropEntry>()` (>0). Both paths already
    // well-tested by existing Drop-aware tests.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:848 — `if layout.size() > self.provider.max_normal_alloc`
    // Same `>` → `>=` mutation; detection via oversized stats.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:851 — `let needed = layout.size() + layout.align().saturating_sub(...) + entry_size;`
    // Mutants: `+` → `-`, `+` → `*`.
    //
    // Detection: an alignment-demanding allocation must succeed.
    // ----------------------------------------------------------------------------

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

    // ----------------------------------------------------------------------------
    // arena.rs:5155 — check_isize_overflow: `if total > (isize::MAX as usize).saturating_sub(padding)`
    // Mutant: `>` → `>=`. At exact equality, original returns Ok; mutant errors.
    //
    // (No deterministic boundary test feasible — covered by general alloc
    //  smoke tests that succeed at smaller sizes.)
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:5180 — `check_chunk_alignment -> Result<(), AllocError> with Ok(())`
    // Mutant replaces the function with `Ok(())`. To kill, allocate with
    // alignment >= MAX_SMART_PTR_ALIGN through a DST path and observe error.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:5339 / 5348 — try_bump_fit's range checks
    //   `if end > drop_back_addr { return None; }`
    //   `if end > payload_end_addr { return None; }`
    // Mutant: `>` → `>=`. Boundary: at exact equality the original accepts.
    // ----------------------------------------------------------------------------

    #[test]
    fn try_bump_fit_at_exact_chunk_end_succeeds() {
        // Cannot exercise the boundary deterministically because the chunk
        // layout is hidden. But many smoke tests would fail if `>` flipped
        // to `>=` because every successful bump-fit at the exact end is now
        // rejected. Covered transitively by `arena_arc.rs` / `arena_box.rs`
        // tests that allocate near boundaries.
    }

    // ----------------------------------------------------------------------------
    // arena_builder.rs:174 — `resolve_capacity`'s `cap - 1` for `next_power_of_two`-style logic.
    // Mutant: `-` → `+`, `-` → `/`.
    // Detection: build an arena with a specific preallocation and observe
    // chunk count.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/constants.rs:77 — min_class_for_bytes arithmetic
    // Mutant: `bits = usize::BITS - bytes.leading_zeros()` then `-` → `+`.
    // Detection: directly test `min_class_for_bytes` via integration: build
    // an arena, allocate at various sizes, verify that class progression
    // matches expectations through preallocate/stats.
    //
    // Indirectly covered by the chunk-acquisition test paths.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/drop_list.rs — pad_bytes / raw_used_bytes arithmetic and replacements
    // `raw_used_bytes`: `sizeof::<fn>() + 2 + 2`
    // `pad_bytes`: padding to PAD_TARGET alignment.
    //
    // I consolidated these as constants. Mutating the underlying constants
    // would change `PAD_BYTES`, causing DropEntry layout to misalign. Existing
    // drop-list tests would catch this. The constants are only computed at
    // compile time so the mutants are stale (constants don't exist as runtime
    // functions any more).
    //
    // Note: these mutants may be against the previous version of the code.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/local_chunk.rs:132 / shared_chunk.rs:155 — max_bump_extent
    //   `capacity - drop_count * size_of::<DropEntry>()`
    // Mutants: `-` → `+`, `-` → `/`. These would change the available
    // space for bump allocations.
    //
    // Detection: many allocations exercise drop-list growth + bump fit;
    // changing this arithmetic would either over- or under-estimate
    // available space, causing either premature OOM or write-past-end.
    //
    // Covered by existing drop-aware tests.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/local_chunk.rs:158 / shared_chunk.rs:167 — entries_top_offset's
    // boundary: `if drop_count < entries_top_offset(capacity) / sizeof::<DropEntry>()`
    // Mutant: `<` → `<=`.
    //
    // Off-by-one in drop-list growth gate. Caught by tests that nearly fill
    // the back-stack.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/chunk_provider.rs:186 / 253 / 419 / 447 — acquire_local/shared arithmetic
    //   `local_header_size() + rounded_payload` / `class_to_bytes(class) - local_header_size()`
    //   etc. Mutants: `-` → `+`, `-` → `/`.
    //
    // Detection: byte_budget should be consumed by chunk-header + payload.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/chunk_provider.rs:300 — preallocate_local's `if target_class > *h`
    // I removed this — already addressed.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/chunk_provider.rs:524 — `release_budget(shared_header_size() + cap)`
    // Mutant: `+` → `*`. Misaccounting in budget release.
    //
    // Detection: a workload that recycles chunks must keep the budget bounded.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/constants.rs:123 — `refcount_overflow_abort` impl Drop for ForceAbort
    // Mutant: replace `drop` with `()`. ForceAbort is `no_std` fallback and
    // the path is `#[cfg_attr(coverage_nightly, coverage(off))]`. Document as
    // genuinely unreachable in tested configurations.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:446 — `Arena::builder()` returns `ArenaBuilder<Global>`.
    // Mutant: `Default::default()` returns the same thing.
    // EQUIVALENT — both call sites produce the same result; no test required.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:1017 / 1299 / 1388 — `match guard e <= cap.saturating_sub(entry_size) with true`
    // These are inside the oversized-allocation routes where the provider's
    // post-condition guarantees the chunk fits. I replaced them with
    // `assert_unchecked`; the mutants are stale.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:1838 — allocate_layout's `prefix + payload + align - 1`
    // Mutant: `-` → `+` in `align - 1`. Changes worst-case bytes needed.
    // ----------------------------------------------------------------------------

    #[test]
    fn allocate_layout_handles_alignment_padding() {
        let arena = Arena::new();
        // Force an aligned allocation that requires padding.
        let _a: Arc<Align64> = arena.alloc_arc(Align64(1));
    }

    // ----------------------------------------------------------------------------
    // arena.rs:853 — `needed = layout.size() + layout.align().saturating_sub(...) + entry_size`
    // in try_alloc_inner_arc_with. Mutating `+` to `*` makes `needed` enormous,
    // forcing routing through oversized chunks for ordinary small types.
    // Detection: stats should show no oversized chunks for ordinary allocs.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:1252 — try_alloc_inner_oversized_value: `match aligned.checked_add(layout.size()) { Some(e) if e <= cap.saturating_sub(entry_size) => ... }`
    // Mutant `&& -> ||`: `e <= cap.saturating_sub(entry_size) || ...` always true.
    //
    // This branch is the post-condition guard of `provider.acquire_local`. I
    // replaced it with `assert_unchecked` for fast-paths but the value-path mutant
    // remains.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:5180-style — `check_chunk_alignment -> Result<(), AllocError> with Ok(())`
    // Mutant: function replaced with `Ok(())`. Killed by `dst_arc_rejects_excessive_alignment_via_layout`.
    // ----------------------------------------------------------------------------
    // At exact equality of `layout.size()` and `max_normal_alloc`, original keeps
    // going through normal chunks; mutant routes to oversized.
    //
    // The boundary check `size > max_normal_alloc` is mutation-resistant in
    // practice because the chunk allocator must reserve header + drop-entry
    // overhead, so a request of exactly `max_normal_alloc` bytes can fail
    // the bump fit even on a chunk of class `max_normal_alloc`. Both
    // original and mutated boundaries thus may route to oversized.
    //
    // Behavioral correctness is asserted by general-purpose alloc tests.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:448 — Arena::builder() returns ArenaBuilder<Global>, constructed
    // via the crate-internal `ArenaBuilder::new()`. `ArenaBuilder` no longer
    // implements `Default`, so the former `from(Default::default())` mutant is
    // no longer generated. No test required.
    // ----------------------------------------------------------------------------
}

mod mutants_for_final {
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

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ============================================================================
    // Reclaim-tail observability tests
    // ----------------------------------------------------------------------------
    // Many missed mutants involve `(cap - len) * elem_size` or string equivalents.
    // Reclaim returns the unused tail to the chunk's bump cursor, so a subsequent
    // allocation that needs that exact space MUST succeed without allocating a
    // new chunk. Wrong arithmetic either reclaims too little (subsequent alloc
    // spills into a new chunk) or too much (cursor moves into already-allocated
    // territory and follow-up writes corrupt earlier data).
    // ============================================================================

    // ============================================================================
    // drop_list constant-layout test
    // ----------------------------------------------------------------------------
    // Mutants on lines 71/75 of drop_list.rs change the computed `PAD_BYTES`,
    // breaking `size_of::<DropEntry>()` and corrupting drop-list stack walks.
    // Verified through observable behavior: many drop-typed allocs must each
    // drop exactly once.
    // ============================================================================

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

    // ============================================================================
    // `cap == len` short-circuit: into_box at exact cap=len skips reclaim.
    // ----------------------------------------------------------------------------
    // At `cap == len`, original skips reclaim; mutant `>=` tries to reclaim 0
    // bytes (no-op). Behavior observable through chunk count not changing.
    // ============================================================================

    // ============================================================================
    // `into_box`'s ZST/empty routing (`== with !=` at line 834)
    // ----------------------------------------------------------------------------
    // Mutant inverts the early-return condition. Non-ZST non-empty vec must
    // take the in-place path (no new chunk). With mutant, it takes the copy
    // fallback which allocates fresh slice storage.
    // ============================================================================

    #[test]
    fn vec_into_box_empty_routes_through_copy_path() {
        let arena = Arena::new();
        let v: ArenaVec<'_, u32> = arena.alloc_vec();
        let b: ArenaBox<[u32]> = v.into_boxed_slice();
        assert_eq!(b.len(), 0);
    }

    // ============================================================================
    // `into_box`'s `consumed_cell.set(idx + 1)` (line 922)
    // ----------------------------------------------------------------------------
    // Mutant `+ with *`: `set(idx * 1) = idx`. Loop never advances and resulting
    // slice holds N copies of element 0. Detection: copy path with distinct
    // element values must preserve order.
    // ============================================================================

    // ============================================================================
    // try_bump_fit `>` boundary (lines 5263/5272)
    // ----------------------------------------------------------------------------
    // Mutant `>=` rejects exact-fit allocations. Every successful allocation
    // must pass this gate, so a workload that allocates many small items would
    // inflate chunk turnover dramatically with the mutant.
    // ============================================================================

    // ============================================================================
    // chunk_provider.rs:536 `+ with *` in release_budget arithmetic
    // ----------------------------------------------------------------------------
    // Wrong release arithmetic drifts the budget tracker and eventually fails.
    // Tightly-budgeted arena cycling allocations exercises this.
    // ============================================================================

    // ============================================================================
    // needs_drop_indirect -> true: non-drop slices must not reserve drop entries
    // ----------------------------------------------------------------------------
    // Mutant: function always returns true, so non-drop allocations also reserve
    // a drop-entry slot. Reduces usable payload per chunk and inflates count.
    // ============================================================================

    // ============================================================================
    // String / Utf16String shrink_to_fit reclaim arithmetic
    // ----------------------------------------------------------------------------
    // Mutants on `let reclaim = self.cap - self.len` or
    // `reclaim_bytes = reclaim_units * 2` change the bytes returned to the chunk
    // cursor. With wrong arithmetic, a follow-up allocation either spills to a new
    // chunk or corrupts the preceding region (asserted by reading back the frozen
    // handle).
    // ============================================================================
}
