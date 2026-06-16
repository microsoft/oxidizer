// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_safety_comment,
    reason = "residue of Rc-test removal: orphaned helpers/imports kept to preserve surrounding test bodies verbatim"
)]

//! Consolidated drop/teardown re-entrancy and drop-behavior regression tests.

mod common;

mod arena_drop_reentrancy {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::unused_result_ok, reason = "ignore fetch_add returns + take() returns in tests")]
    #![allow(unused_must_use, reason = "ignore fetch_add returns in tests")]
    #![allow(unused_results, reason = "test code may discard intermediate results (e.g., fetch_add)")]
    #![allow(clippy::manual_assert, reason = "test code uses if-panic for adversarial-state assertions")]
    #![allow(clippy::items_after_statements, reason = "test helper structs scoped within test bodies")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code; safety is contextual to the scenario")]
    #![allow(clippy::missing_safety_doc, reason = "test code uses local unsafe impls without doc")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
    #[test]
    fn drop_runs_destructors_that_drop_other_smart_pointers() {
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
        {
            let arena = Arena::new();
            let inner = arena.alloc_arc(Inner);
            let _ = arena.alloc(Outer { inner: Some(inner) });
        }
        assert_eq!(OUTER.load(Ordering::SeqCst), 1, "Outer::drop must run");
        assert_eq!(INNER.load(Ordering::SeqCst), 1, "Inner::drop must run");
    }

    #[test]
    fn drop_handles_pinned_chunk_releasing_arc_into_other_chunk() {
        // Reproduces "Issue 1" from the correctness audit: a pinned chunk's
        // drop list contains a value that owns an ArenaArc; dropping the
        // value tears down the Arc's chunk re-entrantly DURING the pinned
        // drain. With the old draining order, the re-entrantly cached
        // chunk would leak silently — destructors all run, but ArenaInner
        // and the chunks linger forever (caught by Miri as a memory leak).
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
        {
            let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
            let inner = arena.alloc_arc(Inner);
            let _ = arena.alloc(Outer { inner: Some(inner) });
            // Force chunk rotation so Outer's chunk goes onto the pinned list.
            let _ = arena.alloc([0_u8; 4000]);
            let _ = arena.alloc([0_u8; 4000]);
            let _ = arena.alloc([0_u8; 4000]);
            let _ = arena.alloc([0_u8; 4000]);
        }
        assert_eq!(OUTER.load(Ordering::SeqCst), 1);
        assert_eq!(INNER.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn no_chunk_leak_under_reentrant_teardown_stress() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Inner;
        impl Drop for Inner {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct Outer {
            inner: Option<Arc<Inner>>,
        }
        impl Drop for Outer {
            fn drop(&mut self) {
                let _ = self.inner.take();
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        for _ in 0..50 {
            let arena = Arena::new();
            let inner = arena.alloc_arc(Inner);
            let _ = arena.alloc(Outer { inner: Some(inner) });
        }
        assert_eq!(COUNT.load(Ordering::SeqCst), 50);
    }

    static REENTERED: AtomicUsize = AtomicUsize::new(0);
    static MARKER_DROPPED: AtomicUsize = AtomicUsize::new(0);

    struct Marker;
    impl Drop for Marker {
        fn drop(&mut self) {
            MARKER_DROPPED.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct EvictThenPanic {
        arena: *const Arena,
    }

    impl Drop for EvictThenPanic {
        fn drop(&mut self) {
            if REENTERED.fetch_add(1, Ordering::SeqCst) == 0 {
                let a = unsafe { &*self.arena };
                let r1 = a.alloc_arc::<[u8; 4000]>([0xAA; 4000]);
                drop(r1);
                let r2 = a.alloc_arc::<[u8; 4000]>([0xBB; 4000]);
                drop(r2);
            }
        }
    }

    #[test]
    fn slice_init_fail_guard_shared_releases_correctly_under_reentrant_eviction() {
        static REENTERED: AtomicUsize = AtomicUsize::new(0);
        static MARKER_DROPPED: AtomicUsize = AtomicUsize::new(0);
        REENTERED.store(0, Ordering::SeqCst);
        MARKER_DROPPED.store(0, Ordering::SeqCst);

        struct ArcMarker;
        impl Drop for ArcMarker {
            fn drop(&mut self) {
                MARKER_DROPPED.fetch_add(1, Ordering::SeqCst);
            }
        }

        struct EvictArcThenPanic {
            arena: *const Arena,
        }
        unsafe impl Send for EvictArcThenPanic {}
        unsafe impl Sync for EvictArcThenPanic {}
        impl Drop for EvictArcThenPanic {
            fn drop(&mut self) {
                if REENTERED.fetch_add(1, Ordering::SeqCst) == 0 {
                    let a = unsafe { &*self.arena };
                    let r1 = a.alloc_arc::<[u8; 4000]>([0xAA; 4000]);
                    drop(r1);
                    let r2 = a.alloc_arc::<[u8; 4000]>([0xBB; 4000]);
                    drop(r2);
                }
            }
        }

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        let m = arena.alloc_arc(ArcMarker);
        drop(m);

        let arena_ptr: *const Arena = &raw const arena;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arena.alloc_slice_fill_with_arc::<EvictArcThenPanic, _>(8, |i| {
                if i == 4 {
                    panic!("planned panic at index 4");
                }
                EvictArcThenPanic { arena: arena_ptr }
            });
        }));
        assert!(result.is_err());

        eprintln!(
            "Shared: REENTERED={}, MARKER_DROPPED={}",
            REENTERED.load(Ordering::SeqCst),
            MARKER_DROPPED.load(Ordering::SeqCst)
        );

        drop(arena);

        eprintln!("Shared AFTER drop: MARKER_DROPPED={}", MARKER_DROPPED.load(Ordering::SeqCst));
        assert_eq!(
            MARKER_DROPPED.load(Ordering::SeqCst),
            1,
            "Shared variant: Marker's chunk was leaked"
        );
    }

    #[test]
    fn refcount_release_guard_typed_shared_releases_correctly_under_reentrant_eviction() {
        static MARKER_DROPPED: AtomicUsize = AtomicUsize::new(0);
        MARKER_DROPPED.store(0, Ordering::SeqCst);

        struct ArcMarker2;
        impl Drop for ArcMarker2 {
            fn drop(&mut self) {
                MARKER_DROPPED.fetch_add(1, Ordering::SeqCst);
            }
        }

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        let m = arena.alloc_arc(ArcMarker2);
        drop(m);

        let arena_ptr: *const Arena = &raw const arena;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arena.alloc_arc_with::<u64, _>(|| {
                let a = unsafe { &*arena_ptr };
                let r1 = a.alloc_arc::<[u8; 4000]>([0xAA; 4000]);
                drop(r1);
                let r2 = a.alloc_arc::<[u8; 4000]>([0xBB; 4000]);
                drop(r2);
                panic!("planned panic in init closure");
            });
        }));
        assert!(result.is_err());

        drop(arena);
        assert_eq!(
            MARKER_DROPPED.load(Ordering::SeqCst),
            1,
            "Typed Shared RefcountReleaseGuard: chunk A leaked, ArcMarker2 not dropped"
        );
    }

    /// Regression for F-002: a reentrant in-chunk alloc from inside an
    /// `alloc_*_with` init closure used to overlap the outer slot's
    /// reservation. The outer write would clobber the inner value, and on
    /// success the outer would roll the bump cursor *backwards* over the
    /// inner value, exposing the slot to subsequent allocations.
    ///
    /// Fix: pre-advance `data_ptr` (and reserve a noop drop entry) before
    /// invoking the user closure, so reentrant allocations land safely past
    /// the outer reservation.
    #[test]
    fn reentrant_in_chunk_alloc_does_not_overlap_outer_slot() {
        use multitude::Arena;
        let arena: Arena = Arena::builder().max_normal_alloc(60 * 1024).with_capacity_local(64 * 1024).build();
        let arena_ptr: *const Arena = &raw const arena;

        // Allocate a u64 via `alloc_with`, and inside the init closure
        // reentrantly allocate a small u64. Both should fit in the same
        // chunk. The outer's value must not be the inner's value.
        let outer = arena.alloc_with::<u64, _>(|| {
            let a = unsafe { &*arena_ptr };
            let inner = a.alloc(0xDEAD_BEEF_u64);
            // The inner reference points at a slot that must NOT overlap
            // the outer's slot. If overlap happened, the outer's later
            // write of 0x1111... would clobber the inner.
            assert_eq!(*inner, 0xDEAD_BEEF_u64);
            0x1111_2222_3333_4444_u64
        });
        assert_eq!(*outer, 0x1111_2222_3333_4444_u64);
        drop(arena);
    }
}

mod alloc_reentrancy {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use core::cell::Cell;

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
}

mod drop_behavior {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::items_after_statements, reason = "test-local types are clearer near use sites")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
}
