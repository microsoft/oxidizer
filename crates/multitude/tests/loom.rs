// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated loom model-checked tests.

#![cfg(loom)]

mod common;

// === merged from tests/loom_arc.rs ===
mod loom_arc {
    #![allow(clippy::std_instead_of_core, reason = "loom + std interop in tests")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use std::sync::atomic::{AtomicUsize as StdAtomicUsize, Ordering as StdOrdering};

    use loom::thread;
    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    /// A payload type that increments a global counter on Drop.  We use a
    /// real `std::sync::atomic::AtomicUsize` here (NOT loom's), because
    /// the counter exists outside the model: it accumulates *across*
    /// permutations to verify each permutation drops exactly once.
    fn drop_counter() -> &'static StdAtomicUsize {
        static C: StdAtomicUsize = StdAtomicUsize::new(0);
        &C
    }

    struct DropCounted;

    impl Drop for DropCounted {
        fn drop(&mut self) {
            let _prev = drop_counter().fetch_add(1, StdOrdering::Relaxed);
        }
    }

    /// Build a fresh `Arena` with a tiny chunk size so chunk allocation
    /// happens deterministically per scenario.
    fn fresh_arena() -> Arena {
        Arena::builder().max_normal_alloc(4 * 1024).build()
    }

    #[test]
    fn arc_clone_drop_race() {
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let c1 = Arc::clone(&original);
            let c2 = Arc::clone(&original);

            let t1 = thread::spawn(move || {
                drop(c1);
            });
            let t2 = thread::spawn(move || {
                drop(c2);
            });

            t1.join().unwrap();
            t2.join().unwrap();

            // Owner-side drop of `original` and `arena`.
            drop(original);
            drop(arena);

            // Exactly one Drop must have run for the payload, regardless
            // of the interleaving Loom picked.
            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1, "DropCounted::drop must run exactly once");
        });
    }

    #[test]
    fn arc_drop_after_arena_drop() {
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let arc: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            // Owner drops the arena BEFORE the worker drops the Arc.
            // Worker's Arc::drop decs ref_count to 0 → runs teardown on
            // the worker (non-owner) thread → decs outstanding_chunks →
            // last reclaimer → free_storage(ArenaInner).
            let t = thread::spawn(move || {
                drop(arc);
            });

            drop(arena);
            t.join().unwrap();

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn two_arcs_two_threads() {
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let c1 = Arc::clone(&original);
            let c2 = Arc::clone(&original);
            // Owner releases its share first; only the worker-thread Arcs remain.
            drop(original);

            let t1 = thread::spawn(move || drop(c1));
            let t2 = thread::spawn(move || drop(c2));

            t1.join().unwrap();
            t2.join().unwrap();

            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn deferred_reconciliation_race() {
        // The owner allocates 2 Arcs on a Shared chunk (so the chunk's
        // arcs_issued is bumped twice non-atomically). The slot evicts
        // when we drop the arena, which does
        // `fetch_sub(LARGE - 2, Release)` on the chunk's atomic
        // `ref_count`. A worker thread is concurrently dropping one of
        // the Arcs (an atomic `fetch_sub(1, Release)`).
        //
        // After both have run and the owner-side `arc2.drop()` has also
        // run, the chunk's net refcount must be zero and teardown must
        // run exactly once.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let arc1: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let arc2: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            let t = thread::spawn(move || drop(arc1));

            // Drop `arc2` and the arena on the owner thread, racing the
            // worker's `arc1` drop.
            drop(arc2);
            drop(arena);

            t.join().unwrap();

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 2, "both DropCounted payloads must drop exactly once");
        });
    }

    #[test]
    fn arc_clone_then_send_then_drop() {
        // Owner clones an Arc and sends it to a worker. Worker drops its
        // clone. Owner drops the original and the arena. Verifies the
        // standard inc/dec ordering produces a correct final count even
        // when the inc and dec happen on different threads.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let cloned = Arc::clone(&original);

            let t = thread::spawn(move || drop(cloned));

            drop(original);
            t.join().unwrap();
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn worker_clones_then_drops() {
        // Owner sends an Arc to a worker. The worker clones it on its
        // own thread, drops the clone, then drops the original. The
        // owner drops the arena. Verifies clone-on-non-owner-thread is
        // sound (the inc + later dec both happen on the worker, but
        // they're paired across the spawned thread boundary, exercising
        // Loom's HB tracking).
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            let t = thread::spawn(move || {
                let cloned = Arc::clone(&original);
                drop(cloned);
                drop(original);
            });

            // Owner waits for worker, then drops the arena.
            t.join().unwrap();
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn arena_drop_concurrent_with_clone_and_drop() {
        // Three concurrent operations: owner drops the arena while two
        // workers are racing on Arc clone/drop. Stresses the
        // `arena_dropped` Acquire/Release pairing on cross-thread
        // teardown.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let c1 = Arc::clone(&original);

            let t1 = thread::spawn(move || drop(c1));
            // Owner drops original then arena while t1 races.
            drop(original);
            drop(arena);
            t1.join().unwrap();

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn two_workers_clone_and_drop_during_eviction() {
        // Eviction race: owner evicts a Shared chunk via `reset` while two
        // workers drop their Arcs. The reconcile must produce a refcount
        // that reaches 0 exactly once.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let mut arena = fresh_arena();
            let arc1: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let arc2: Arc<DropCounted> = Arc::clone(&arc1);

            let t1 = thread::spawn(move || drop(arc1));
            let t2 = thread::spawn(move || drop(arc2));

            arena.reset();

            t1.join().unwrap();
            t2.join().unwrap();
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1, "Drop must run exactly once");
        });
    }

    #[test]
    fn worker_drop_racing_eviction_then_owner_drops_arena() {
        // Variant of `deferred_reconciliation_race`: the arena is dropped
        // after the eviction, so the worker's drop hits the
        // `outstanding_chunks` last-reclaimer path on the now-detached chunk.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let mut arena = fresh_arena();
            let arc1: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            let t = thread::spawn(move || drop(arc1));

            arena.reset();
            drop(arena);

            t.join().unwrap();

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn arena_drop_with_active_workers_and_chunk_cache_reuse() {
        // Owner allocates an Arc, resets (chunk cached), allocates again
        // (cache pop revives), all while a worker drops the first Arc.
        // Stresses the cache-revive path against in-flight worker drops.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let mut arena = fresh_arena();
            let arc1: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            let t = thread::spawn(move || drop(arc1));

            arena.reset();
            let arc2: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            drop(arc2);
            drop(arena);

            t.join().unwrap();

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 2, "both DropCounted payloads must drop exactly once");
        });
    }

    #[test]
    fn arc_clone_racing_drop_same_arc() {
        // Thread A clones an Arc while thread B drops a different clone
        // of the same chunk's Arc concurrently. The classic clone/drop
        // window: `fetch_add(Relaxed)` on one thread races with
        // `fetch_sub(Release) + fence(Acquire)` on the other.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let dropper_arc = Arc::clone(&original);
            let cloner_arc = Arc::clone(&original);
            drop(original);

            let t_a = thread::spawn(move || {
                let cloned = Arc::clone(&cloner_arc);
                drop(cloned);
                drop(cloner_arc);
            });
            let t_b = thread::spawn(move || drop(dropper_arc));

            t_a.join().unwrap();
            t_b.join().unwrap();
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn deferred_reconciliation_two_workers_drop() {
        // Owner allocates 3 Arcs (so `arcs_issued = 3` non-atomically),
        // then evicts via arena drop (`fetch_sub(LARGE - 3, Release)`).
        // Two workers each drop one Arc concurrently with the eviction;
        // the third is dropped on the owner thread. Tests that the
        // reconciliation arithmetic stays correct with multiple workers
        // racing the eviction.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let arc1: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let arc2: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let arc3: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            let t1 = thread::spawn(move || drop(arc1));
            let t2 = thread::spawn(move || drop(arc2));

            drop(arc3);
            drop(arena);

            t1.join().unwrap();
            t2.join().unwrap();

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 3);
        });
    }

    #[test]
    fn arena_reset_concurrent_with_clone_and_drop() {
        // Owner calls `arena.reset()` (NOT drop) while two workers race
        // on Arc clone/drop. `reset` evicts in-place rather than tearing
        // down `ArenaInner`, so the orderings exercised differ from the
        // arena-drop case.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let mut arena = fresh_arena();
            let original: Arc<DropCounted> = arena.alloc_arc(DropCounted);
            let c1 = Arc::clone(&original);
            drop(original);

            let t = thread::spawn(move || drop(c1));

            arena.reset();
            t.join().unwrap();
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 1);
        });
    }

    #[test]
    fn cache_pop_concurrent_with_prior_generation_worker_drop() {
        // Owner allocates an Arc on chunk-gen-1, resets (chunk cached),
        // then allocates a new Arc — which pops the cached chunk and
        // re-initializes it (gen-2). Concurrently, a worker holding the
        // gen-1 Arc drops it, hitting the now-revived chunk's refcount.
        // Tests that cache-revive races a teardown decrement on the
        // prior generation safely.
        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let mut arena = fresh_arena();
            let arc_gen1: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            let t = thread::spawn(move || drop(arc_gen1));

            arena.reset();
            let arc_gen2: Arc<DropCounted> = arena.alloc_arc(DropCounted);

            t.join().unwrap();
            drop(arc_gen2);
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(after - baseline, 2);
        });
    }

    #[test]
    fn arc_str_clone_drop_race() {
        // Same pattern as `arc_clone_drop_race` but on the str path.
        // ArcStr uses the same atomic refcount machinery as Arc<T>
        // (fetch_add/Relaxed + fetch_sub/Release + Acquire fence) but
        // through a different allocation/teardown shape (no DropEntry,
        // payload is non-Drop bytes). Verifies the str path's orderings.
        loom::model(|| {
            let arena = fresh_arena();
            let original = arena.alloc_str_arc("loom");
            let c1 = original.clone();
            let c2 = original.clone();

            let t1 = thread::spawn(move || drop(c1));
            let t2 = thread::spawn(move || drop(c2));

            t1.join().unwrap();
            t2.join().unwrap();

            drop(original);
            drop(arena);
        });
    }

    #[test]
    fn shared_cache_push_push_race() {
        // Two workers concurrently drop the last `Arc` on two different
        // shared chunks. Both drops route through `release_shared`, which
        // pushes the eligible chunk onto the provider's Treiber-stack
        // `shared_cache_head` via `compare_exchange_weak(AcqRel/Acquire)`.
        // Loom explores every interleaving of the two `push_shared_cache`
        // CAS retry loops, including the case where the first thread's
        // CAS observes `head` modified by the second's push and has to
        // re-store its `next` pointer.
        loom::model(|| {
            let arena = fresh_arena();
            // Each `Arc<[u32; 256]>` takes 1 KiB + drop entry; with
            // `max_normal_alloc = 4 KiB` chunks, two of these allocate in
            // separate chunks via refill, so dropping each on a different
            // worker forces two independent `push_shared_cache` paths.
            let a: Arc<[u32; 256]> = arena.alloc_arc([0_u32; 256]);
            let b: Arc<[u32; 256]> = arena.alloc_arc([0_u32; 256]);

            let t1 = thread::spawn(move || drop(a));
            let t2 = thread::spawn(move || drop(b));

            t1.join().unwrap();
            t2.join().unwrap();

            // Owner allocates again — exercises `try_pop_shared_at_least`
            // popping a chunk just pushed by the workers. Single-consumer
            // pop is the owner thread; loom orders the pop strictly after
            // the joins, but the pop's read of `head.next` must still see
            // the value the most recent pusher settled via its Release CAS.
            let c: Arc<[u32; 256]> = arena.alloc_arc([0_u32; 256]);
            drop(c);
            drop(arena);
        });
    }

    #[test]
    fn arc_assume_init_cross_thread_clones() {
        // Two workers each call `Arc::<MaybeUninit<T>>::assume_init` on
        // their own clone of the same allocation. Both `store_drop_fn`s
        // race on the chunk's `InnerDropEntry::drop_fn` atomic. The writes
        // are idempotent (same `drop_shim_one::<T>` pointer), but loom
        // verifies the Release/Acquire chain through `drop_count`
        // (publishing the entry slot from owner) and `refcount` (Acquire
        // fence on last decrement -> Relaxed load of `drop_fn` during
        // chunk-replay) holds under every reordering.
        //
        // `AssumeInitDropped` has all-zero-bits as a valid pattern (a
        // single `u64` field), so `alloc_zeroed_arc` produces a
        // legitimately-initialized value that `assume_init` can soundly
        // consume.
        struct AssumeInitDropped {
            _bytes: u64,
        }
        impl Drop for AssumeInitDropped {
            fn drop(&mut self) {
                drop_counter().fetch_add(1, StdOrdering::Relaxed);
            }
        }

        loom::model(|| {
            let baseline = drop_counter().load(StdOrdering::Relaxed);

            let arena = fresh_arena();
            let arc: Arc<core::mem::MaybeUninit<AssumeInitDropped>> = arena.alloc_zeroed_arc();
            let cloned = Arc::clone(&arc);

            let t1 = thread::spawn(move || {
                // SAFETY: all-zero bits is a valid `AssumeInitDropped`.
                let init: Arc<AssumeInitDropped> = unsafe { arc.assume_init() };
                drop(init);
            });
            let t2 = thread::spawn(move || {
                // SAFETY: all-zero bits is a valid `AssumeInitDropped`.
                let init: Arc<AssumeInitDropped> = unsafe { cloned.assume_init() };
                drop(init);
            });

            t1.join().unwrap();
            t2.join().unwrap();
            drop(arena);

            let after = drop_counter().load(StdOrdering::Relaxed);
            assert_eq!(
                after - baseline,
                1,
                "Drop runs exactly once even with two cross-thread assume_inits"
            );
        });
    }
}

// === merged from tests/loom_bytesbuf.rs ===
mod loom_bytesbuf {
    #![cfg(all(loom, feature = "bytesbuf"))]
    #![allow(clippy::std_instead_of_core, reason = "loom + std interop in tests")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use bytesbuf::mem::Memory;
    use loom::thread;
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn bytesbuf_view_clone_drop_race() {
        // Owner reserves an arena-backed `BytesBuf`, fills it, consumes it
        // into a view, clones the view, and sends the original + clone to
        // two workers that drop them concurrently. Each drop runs
        // `BlockRef::drop` -> `ArenaBlockState::ref_count.fetch_sub(1, Release)`;
        // the last decrement does `fence(Acquire)` and then frees the
        // backing chunk hold. Loom verifies the chain produces exactly one
        // chunk release regardless of which thread sees the last
        // decrement.
        loom::model(|| {
            let arena = Arena::new();
            let mut buf = arena.reserve(16);
            buf.put_slice(*b"hello");
            let view_a = buf.consume_all();
            let view_b = view_a.clone();

            let t1 = thread::spawn(move || drop(view_a));
            let t2 = thread::spawn(move || drop(view_b));

            t1.join().unwrap();
            t2.join().unwrap();
            drop(arena);
        });
    }

    #[test]
    fn bytesbuf_view_clone_in_worker_drop_split() {
        // Owner sends a view to worker A; worker A clones it on its own
        // thread and sends the clone to worker B. Then both workers drop
        // their views. Exercises clone-on-non-owner-thread (refcount
        // increment from one worker) paired with cross-thread drops.
        loom::model(|| {
            let arena = Arena::new();
            let mut buf = arena.reserve(8);
            buf.put_slice(*b"loom");
            let view = buf.consume_all();

            let t1 = thread::spawn(move || {
                let cloned = view.clone();
                cloned
            });
            let cloned = t1.join().unwrap();

            let t2 = thread::spawn(move || drop(cloned));
            t2.join().unwrap();

            drop(arena);
        });
    }
}
