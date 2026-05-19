// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated drop/teardown re-entrancy and drop-behavior regression tests.

mod common;

// === merged from tests/arena_drop_reentrancy.rs ===
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

    /// Regression test for the bump-cursor-advancement OOB write found by
    /// the 2026-05-01 correctness audit (Issue 1).
    ///
    /// Scenario: in the slow path of an arena allocation, the arena evicts
    /// the current chunk from `current_local`. If that chunk's only refcount
    /// was the slot's transient +1 (no surviving smart pointers, not
    /// pinned), its `OwnedChunk::drop` runs every linked `drop_fn`. A user
    /// `Drop` impl that re-enters `arena.alloc_*` lands on the populated
    /// new-chunk slot via the fast path and bumps the new chunk's bump
    /// cursor.
    ///
    /// Before the fix, `try_get_chunk_for_local` dropped that evicted-chunk
    /// guard at function epilogue — i.e., **before** the outer caller's
    /// post-acquisition `*_unchecked` allocation. The unchecked alloc then
    /// trusted its pre-eviction fit-check (which used the new chunk's
    /// pre-re-entrancy cursor) and wrote past the bytes the re-entrant
    /// alloc had already claimed, eventually overflowing the chunk past
    /// `total_size` — undefined behavior.
    ///
    /// The fix returns the eviction guard from `try_get_chunk_for_*` so the
    /// caller binds it; it now drops AFTER the unchecked alloc, so re-entrant
    /// allocs claim cursor bytes only beyond the outer alloc's range.
    ///
    /// This test reproduces the audit's worked example: a small builder
    /// configuration (`chunk_size = 8 KiB`, `max_normal_alloc = 4 KiB`),
    /// a `Bomb` whose `Drop` allocates a 4000-byte buffer, and two
    /// post-Bomb 4000-byte allocations. The second forces an eviction
    /// whose guard-drop re-enters via `Bomb`. With the bug, the third
    /// allocation would write 4000 bytes past offset 4256 in an 8192-byte
    /// chunk, overflowing by 64 bytes; the test asserts non-overlapping
    /// allocations and successful completion.
    #[test]
    fn slow_path_eviction_does_not_advance_bump_cursor_via_reentrant_drop() {
        use allocator_api2::alloc::Global;
        use multitude::Arena;

        static BOMB_ALLOC: AtomicUsize = AtomicUsize::new(0);

        // Bomb allocates a large array in its Drop, capturing the result's
        // address so the test can verify non-overlap with the outer alloc.
        struct Bomb {
            arena: *const Arena<Global>,
        }
        impl Drop for Bomb {
            fn drop(&mut self) {
                // SAFETY: the arena outlives every linked drop_fn; the test
                // holds the arena until after this drop runs.
                let arena = unsafe { &*self.arena };
                let buf: &mut [u8; 4000] = arena.alloc([0xCC; 4000]);
                BOMB_ALLOC.store(buf.as_ptr() as usize, Ordering::SeqCst);
            }
        }

        BOMB_ALLOC.store(0, Ordering::SeqCst);

        // 8 KiB chunks with a 4 KiB cap on per-allocation size. With these,
        // two 4 KiB allocations don't fit in a single 8 KiB chunk minus
        // header, so the second forces an eviction.
        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        // Step 1: allocate Bomb via `alloc_rc`. `alloc_rc` does NOT pin the
        // chunk (smart-pointer allocs use `pin_for_bump = false`). After
        // dropping the Rc, the chunk's refcount is 1 (slot's transient
        // hold). When that chunk is later evicted with non-pinned status,
        // its `OwnedChunk::drop` will run every linked drop_fn — including
        // `Bomb::drop`.
        {
            let bomb = arena.alloc_rc(Bomb { arena: &raw const arena });
            drop(bomb);
        }

        // Step 2: fill the chunk's remaining room with a non-pinning alloc
        // big enough that one more 4000-byte allocation can't fit. After
        // dropping the Rc, the chunk's refcount returns to 1 and remains
        // non-pinned.
        {
            let filler = arena.alloc_rc::<[u8; 4000]>([0xDD; 4000]);
            drop(filler);
        }

        // Step 3: trigger the slow path. `arena.alloc::<[u8; 4000]>` uses
        // `pin_for_bump = true`. Its fast path misses (the chunk is nearly
        // full); its slow path enters `try_get_chunk_for_local`, which
        // evicts the current chunk. Eviction takes the non-pinned chunk
        // into the eviction guard, then installs a fresh, pinned chunk D.
        //
        // With the bug, the eviction guard would drop *before* the outer
        // caller's `alloc_unchecked` runs: that drop tears down the
        // evicted chunk, runs Bomb::drop, which re-enters `arena.alloc`,
        // hits the fast path on D, and bumps D's cursor by 4000. The outer
        // caller's `alloc_unchecked` then writes 4000 bytes starting at
        // the post-bomb cursor — overflowing the 8 KiB chunk by ~64 bytes
        // (UB).
        //
        // With the fix, the outer `alloc_unchecked` runs first, claiming
        // its 4000 bytes from D's fresh cursor. The eviction guard then
        // drops; Bomb's reentrant alloc misses D's fast path (D is
        // already 4000 bytes deep) and goes to its own slow path,
        // installing a fresh chunk E for Bomb's allocation. No overlap,
        // no OOB.
        let outer: &mut [u8; 4000] = arena.alloc([0xBB; 4000]);
        let outer_start = outer.as_ptr() as usize;
        let outer_end = outer_start + outer.len();

        let bomb_start = BOMB_ALLOC.load(Ordering::SeqCst);
        assert!(bomb_start != 0, "Bomb::drop should have run during slow-path eviction");
        let bomb_end = bomb_start + 4000;

        // The outer allocation and the bomb's allocation must not overlap.
        // With the bug, they would (or the outer would write past the
        // chunk).
        assert!(
            outer_end <= bomb_start || bomb_end <= outer_start,
            "outer [{outer_start:#x}, {outer_end:#x}) overlaps bomb [{bomb_start:#x}, {bomb_end:#x})"
        );

        // Confirm the outer allocation's bytes weren't clobbered by a
        // re-entrant write into its range (which would happen if the
        // eviction guard's drop ran in the wrong order and the
        // `alloc_unchecked` then bumped past Bomb's claim).
        assert!(outer.iter().all(|&v| v == 0xBB), "outer was overwritten");
    }

    /// Companion to the prior test: same staging, but the OUTER allocation
    /// is `alloc_rc::<[u8; 4000]>` — a non-pinning slot allocation
    /// (`pin_for_bump = false`). With this shape the re-entrant `Bomb::drop`
    /// can not only advance the new chunk's bump cursor — it can EVICT the
    /// freshly-installed chunk, drop its `OwnedChunk`, and (because no
    /// caller has yet bumped the chunk's refcount above the slot's
    /// transient +1) tear it down to refcount 0, sending it to the cache
    /// or freeing it. With the bug, `try_reserve_and_init_aligned`'s
    /// `_evicted_guard` drops at the end of its `else` block — *before*
    /// the outer caller writes the value, links the drop entry, or calls
    /// `inc_ref_for` — so all of that work then targets a chunk that is no
    /// longer logically allocated. The result is a refcount underflow
    /// (later double-free) and/or a write to memory that has since been
    /// reclaimed.
    #[test]
    fn slow_path_eviction_does_not_free_new_chunk_via_reentrant_drop_in_pin_false_smart_ptr_paths() {
        use allocator_api2::alloc::Global;
        use multitude::Arena;

        static BOMB_ALLOC: AtomicUsize = AtomicUsize::new(0);

        struct Bomb {
            arena: *const Arena<Global>,
        }
        impl Drop for Bomb {
            fn drop(&mut self) {
                // SAFETY: the test holds the arena alive across this Drop.
                let arena = unsafe { &*self.arena };
                // A 4000-byte alloc that won't fit alongside a previous
                // 4000-byte alloc in an 8 KiB chunk — forces re-entrant
                // slow path which evicts whatever's currently in
                // `current_local`.
                let r = arena.alloc_rc::<[u8; 4000]>([0xCC; 4000]);
                BOMB_ALLOC.store(r.as_ptr() as usize, Ordering::SeqCst);
                drop(r);
            }
        }

        BOMB_ALLOC.store(0, Ordering::SeqCst);

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        // Stage Bomb in chunk A (and immediately drop the Rc — the chunk
        // still owns the linked drop entry until A tears down).
        {
            let bomb = arena.alloc_rc(Bomb { arena: &raw const arena });
            drop(bomb);
        }
        // Fill A so the next 4000-byte alloc misses A's fast path.
        {
            let filler = arena.alloc_rc::<[u8; 4000]>([0xDD; 4000]);
            drop(filler);
        }

        // Outer non-pinning allocation: triggers the slow path, evicts A
        // (with Bomb's drop entry), installs a fresh chunk B at refcount=1
        // (slot's transient hold). With the bug, B's `_evicted_guard`
        // drops at the end of the inner `else` block, runs Bomb::drop,
        // which re-enters `alloc_rc::<[u8; 4000]>`. That re-entrant call
        // also misses B's fast path (B already used 4000 bytes? actually
        // it doesn't — B was just installed and the outer caller hasn't
        // yet alloc-unchecked-claimed). Either way, the re-entrant slow
        // path can install a third chunk C and let B fall to refcount 0,
        // freeing it. Outer then proceeds to `value.write` / `inc_ref_for`
        // on a stale B — UAF / refcount underflow.
        let outer = arena.alloc_rc::<[u8; 4000]>([0xBB; 4000]);

        // The bomb must have run.
        assert!(
            BOMB_ALLOC.load(Ordering::SeqCst) != 0,
            "Bomb::drop should have run during slow-path eviction"
        );

        // The outer Rc's payload should still be intact.
        assert!(outer.iter().all(|&v| v == 0xBB), "outer Rc payload was overwritten");

        // Drop the Rc and the arena cleanly. With the bug, this would
        // double-free or trigger heap corruption.
        drop(outer);
        drop(arena);
    }

    /// Verify that re-entrant `Drop`/`Clone` runs INSIDE `alloc_rc_with`'s
    /// init closure cannot UAF the freshly-installed chunk. The closure
    /// runs after `try_bump_alloc_in_current` has reserved bytes but
    /// BEFORE `inc_ref_for_normal` lifts the chunk's refcount above the
    /// slot's transient +1; if the closure triggers an eviction (e.g. via
    /// another large alloc), the chunk's transient +1 transfers to an
    /// eviction guard which can drop to 0 and tear down the chunk —
    /// leaving us writing into freed memory.
    #[test]
    fn reentrant_init_closure_does_not_free_new_chunk() {
        use multitude::Arena;

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        // Stage chunk A in current_local (slot's +1 is its only refcount
        // after dropping the Rc).
        let stage = arena.alloc_rc(0_u8);
        drop(stage);

        let arena_ptr: *const Arena = &raw const arena;
        let r = arena.alloc_rc_with::<u64, _>(|| {
            // SAFETY: arena outlives the closure (held by the test frame).
            let a = unsafe { &*arena_ptr };
            // Two 4 KiB allocs that don't fit alongside each other in
            // 8 KiB minus header — second one forces an eviction, which
            // drops the previous chunk's +1 (the chunk we're meant to be
            // initializing into).
            let r1 = a.alloc_rc::<[u8; 4000]>([0xAA; 4000]);
            drop(r1);
            let r2 = a.alloc_rc::<[u8; 4000]>([0xBB; 4000]);
            drop(r2);
            0xDEAD_BEEF_u64
        });
        assert_eq!(*r, 0xDEAD_BEEF_u64);
        drop(r);
        drop(arena);
    }

    /// Same UAF window as `reentrant_init_closure_does_not_free_new_chunk`
    /// but exercised through `alloc_slice_fill_with_rc`'s init closure.
    /// `acquire_slice_slot` returns a chunk whose only refcount is the
    /// slot's transient `+1` (Normal+Local) and then the caller runs
    /// `f(i)` for each element; if `f(i)` re-enters and evicts the slot,
    /// the chunk is freed before `commit_slice_init` runs.
    #[test]
    fn reentrant_fill_with_closure_does_not_free_new_chunk() {
        use multitude::Arena;

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        let stage = arena.alloc_rc(0_u8);
        drop(stage);

        let arena_ptr: *const Arena = &raw const arena;
        let r = arena.alloc_slice_fill_with_rc::<u64, _>(8, |i| {
            if i == 0 {
                // Re-entrancy on the FIRST element; force eviction of
                // the chunk we're filling.
                // SAFETY: arena outlives the closure (held by test frame).
                let a = unsafe { &*arena_ptr };
                let r1 = a.alloc_rc::<[u8; 4000]>([0xAA; 4000]);
                drop(r1);
                let r2 = a.alloc_rc::<[u8; 4000]>([0xBB; 4000]);
                drop(r2);
            }
            i as u64
        });
        assert_eq!(&*r, &[0, 1, 2, 3, 4, 5, 6, 7]);
        drop(r);
        drop(arena);
    }

    /// Same as above but for `alloc_slice_clone_rc` — re-entrancy from
    /// `T::clone()`.
    #[test]
    fn reentrant_clone_does_not_free_new_chunk() {
        use std::sync::Mutex;

        use multitude::Arena;

        struct Reenter {
            arena: *const Arena,
            first: Mutex<bool>,
        }
        impl Clone for Reenter {
            fn clone(&self) -> Self {
                let take = {
                    let mut first = self.first.lock().unwrap();
                    let take = *first;
                    *first = false;
                    take
                };
                if take {
                    // SAFETY: arena outlives the clone (held by test frame).
                    let a = unsafe { &*self.arena };
                    let r1 = a.alloc_rc::<[u8; 4000]>([0xAA; 4000]);
                    drop(r1);
                    let r2 = a.alloc_rc::<[u8; 4000]>([0xBB; 4000]);
                    drop(r2);
                }
                Self {
                    arena: self.arena,
                    first: Mutex::new(false),
                }
            }
        }

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        let stage = arena.alloc_rc(0_u8);
        drop(stage);

        let src = [Reenter {
            arena: &raw const arena,
            first: Mutex::new(true),
        }];
        let r = arena.alloc_slice_clone_rc(&src);
        drop(r);
        drop(arena);
    }

    /// Slice-path counterpart to
    /// `slow_path_eviction_does_not_free_new_chunk_via_reentrant_drop_in_pin_false_smart_ptr_paths`.
    ///
    /// Since `acquire_slice_slot_slow` now drops its `EvictedChunkGuard`
    /// *internally* (right after `take_protective_plus_one`, before
    /// returning to the caller), the protective `+1` must be already on
    /// the freshly installed chunk by the time the evicted chunk's
    /// teardown runs. If a previous chunk's deferred `Drop` re-enters the
    /// arena and triggers another slow-path allocation that evicts our
    /// new chunk, the `+1` must keep our chunk alive until the slice init
    /// loop and `commit_slice_init` complete.
    #[test]
    fn slow_path_eviction_does_not_free_new_chunk_via_reentrant_drop_in_slice_paths() {
        use allocator_api2::alloc::Global;
        use multitude::Arena;

        static BOMB_ALLOC: AtomicUsize = AtomicUsize::new(0);

        struct Bomb {
            arena: *const Arena<Global>,
        }
        impl Drop for Bomb {
            fn drop(&mut self) {
                // SAFETY: the test holds the arena alive across this Drop.
                let arena = unsafe { &*self.arena };
                // A 4000-byte alloc that won't fit alongside a previous
                // 4000-byte alloc in an 8 KiB chunk — forces re-entrant
                // slow path which evicts whatever's currently in
                // `current_local`.
                let r = arena.alloc_rc::<[u8; 4000]>([0xCC; 4000]);
                BOMB_ALLOC.store(r.as_ptr() as usize, Ordering::SeqCst);
                drop(r);
            }
        }

        BOMB_ALLOC.store(0, Ordering::SeqCst);

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        // Stage Bomb in chunk A.
        {
            let bomb = arena.alloc_rc(Bomb { arena: &raw const arena });
            drop(bomb);
        }
        // Fill A so the next slice alloc misses A's fast path.
        {
            let filler = arena.alloc_rc::<[u8; 4000]>([0xDD; 4000]);
            drop(filler);
        }

        // Outer slice alloc: triggers the slice slow path
        // (`acquire_slice_slot_slow`), which evicts A. With the new
        // timing, `evicted_guard` drops INSIDE
        // `acquire_slice_slot_slow` — right after the protective +1
        // is taken. Bomb's Drop then runs, allocating `[u8; 4000]`,
        // which itself evicts the freshly installed chunk B. Without
        // the protective +1, B falls to refcount 0 and is freed,
        // leaving the outer slice path writing into reclaimed memory.
        let outer = arena.alloc_slice_clone_rc::<u8>(&[0xBB; 4000]);

        assert!(
            BOMB_ALLOC.load(Ordering::SeqCst) != 0,
            "Bomb::drop should have run during slice slow-path eviction"
        );

        assert!(outer.iter().all(|&v| v == 0xBB), "outer slice payload was overwritten");

        drop(outer);
        drop(arena);
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
                let r1 = a.alloc_rc::<[u8; 4000]>([0xAA; 4000]);
                drop(r1);
                let r2 = a.alloc_rc::<[u8; 4000]>([0xBB; 4000]);
                drop(r2);
            }
        }
    }

    #[test]
    fn slice_init_fail_guard_releases_correctly_under_reentrant_eviction() {
        REENTERED.store(0, Ordering::SeqCst);
        MARKER_DROPPED.store(0, Ordering::SeqCst);

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        // Put a Marker in chunk A so we can detect leak: if A doesn't tear
        // down, MARKER_DROPPED stays at 0.
        let marker_rc = arena.alloc_rc(Marker);
        drop(marker_rc);

        let arena_ptr: *const Arena = &raw const arena;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arena.alloc_slice_fill_with_rc::<EvictThenPanic, _>(8, |i| {
                if i == 4 {
                    panic!("planned panic at index 4");
                }
                EvictThenPanic { arena: arena_ptr }
            });
        }));
        assert!(result.is_err());

        eprintln!(
            "REENTERED={}, MARKER_DROPPED={}",
            REENTERED.load(Ordering::SeqCst),
            MARKER_DROPPED.load(Ordering::SeqCst)
        );

        drop(arena);

        eprintln!("AFTER drop: MARKER_DROPPED={}", MARKER_DROPPED.load(Ordering::SeqCst));
        assert_eq!(
            MARKER_DROPPED.load(Ordering::SeqCst),
            1,
            "Marker's chunk was leaked - drop didn't run"
        );
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
    fn refcount_release_guard_typed_releases_correctly_under_reentrant_eviction() {
        static MARKER_DROPPED: AtomicUsize = AtomicUsize::new(0);
        MARKER_DROPPED.store(0, Ordering::SeqCst);

        struct Marker2;
        impl Drop for Marker2 {
            fn drop(&mut self) {
                MARKER_DROPPED.fetch_add(1, Ordering::SeqCst);
            }
        }

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();

        // Marker into chunk A, dropped immediately so its drop entry stays
        // in A's list; runs only at A's teardown.
        let m = arena.alloc_rc(Marker2);
        drop(m);

        let arena_ptr: *const Arena = &raw const arena;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arena.alloc_rc_with::<u64, _>(|| {
                // SAFETY: arena outlives.
                let a = unsafe { &*arena_ptr };
                let r1 = a.alloc_rc::<[u8; 4000]>([0xAA; 4000]);
                drop(r1);
                let r2 = a.alloc_rc::<[u8; 4000]>([0xBB; 4000]);
                drop(r2);
                panic!("planned panic in init closure");
            });
        }));
        assert!(result.is_err());

        drop(arena);
        assert_eq!(
            MARKER_DROPPED.load(Ordering::SeqCst),
            1,
            "Typed RefcountReleaseGuard: chunk A leaked, Marker2 not dropped"
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

    /// Regression for the `Vec::into_arena_rc` slow-path UAF: re-entrant
    /// `Drop` (the Vec's old buffer deallocate's drop chain) running
    /// BETWEEN `reserve_slice` and `commit_slice` could free the
    /// freshly-installed reservation chunk before commit linked the drop
    /// entry and bumped the smart pointer's refcount.
    #[test]
    fn vec_into_arena_rc_basic_smoke() {
        use multitude::Arena;
        use multitude::vec::Vec as ArenaVec;

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let mut v = ArenaVec::<u8>::new_in(&arena);
        for i in 0..16 {
            v.push(i);
        }
        let frozen = v.into_arena_rc();
        assert_eq!(frozen.len(), 16);
        drop(frozen);
        drop(arena);
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

// === merged from tests/alloc_reentrancy.rs ===
mod alloc_reentrancy {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use core::cell::Cell;

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn reentrant_alloc_from_drop_during_eviction() {
        struct DropAlloc<'a> {
            arena: &'a Arena,
            fired: &'a Cell<usize>,
        }
        impl Drop for DropAlloc<'_> {
            fn drop(&mut self) {
                self.fired.set(self.fired.get() + 1);
                // Re-entrant allocation while the outer alloc is mid-eviction.
                let _r = self.arena.alloc_rc(42_u32);
            }
        }

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let fired = Cell::new(0);

        {
            let r = arena.alloc_rc(DropAlloc {
                arena: &arena,
                fired: &fired,
            });
            // Release the outer Rc; the entry remains linked into the chunk's
            // drop list (Rc only released the slot's +1).
            drop(r);

            // Allocate enough small smart-pointer values to force the chunk
            // holding our `DropAlloc` entry to be evicted from `current_local`.
            for _ in 0..200 {
                let r = arena.alloc_rc([0_u8; 100]);
                drop(r);
            }
        }
        // Drop runs at arena teardown; the inner re-entrant alloc must succeed
        // without panicking and without leaking chunks.
        assert!(fired.get() >= 1);
    }
}

// === merged from tests/drop_behavior.rs ===
mod drop_behavior {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::items_after_statements, reason = "test-local types are clearer near use sites")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn alloc_drop_runs_at_chunk_teardown() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Counter;
        impl Drop for Counter {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        {
            let _a = arena.alloc_rc(Counter);
            let _b = arena.alloc_rc(Counter);
            let _c = arena.alloc_rc(Counter);
            assert_eq!(COUNT.load(Ordering::SeqCst), 0);
        }
        drop(arena);
        assert_eq!(COUNT.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn drops_in_lifo_order() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(std::vec::Vec::new()));
        struct Logger {
            id: u32,
            log: std::sync::Arc<std::sync::Mutex<std::vec::Vec<u32>>>,
        }
        impl Drop for Logger {
            fn drop(&mut self) {
                self.log.lock().unwrap().push(self.id);
            }
        }

        let arena = Arena::new();
        let a = arena.alloc_rc(Logger { id: 1, log: log.clone() });
        let b = arena.alloc_rc(Logger { id: 2, log: log.clone() });
        let c = arena.alloc_rc(Logger { id: 3, log: log.clone() });
        drop(a);
        drop(b);
        drop(c);
        drop(arena);
        assert_eq!(*log.lock().unwrap(), vec![3, 2, 1]);
    }

    #[test]
    fn handles_keep_arena_storage_alive() {
        // The arena drops, but live smart pointers keep their chunk's backing
        // storage alive.
        let s = {
            let arena = Arena::new();
            arena.alloc_rc(std::string::String::from("survives the arena"))
        };
        assert_eq!(*s, "survives the arena");
    }
}
