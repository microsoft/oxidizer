// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for Simple-reference allocations: `Arena::alloc`, `alloc_str`,
//! and the slice variants. These return `&'arena mut T` whose lifetime
//! is tied to the arena reference, with no per-pointer refcount.
//!
//! The chunk that hosts each value is "pinned" so it survives past
//! chunk rotation. The pinning costs one bit per chunk and is released
//! at arena drop.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
#![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
#![allow(clippy::cast_possible_truncation, reason = "test data is small")]
#![allow(clippy::needless_range_loop, reason = "test indexing is intentional")]
#![allow(clippy::missing_asserts_for_indexing, reason = "test code")]
#![allow(clippy::redundant_clone, reason = "test code")]
#![allow(clippy::needless_lifetimes, reason = "explicit lifetimes clarify the test's intent")]
#![allow(clippy::assertions_on_result_states, reason = "tests assert error returns")]
#![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
#![allow(unused_results, reason = "test code")]

mod common;

use multitude::Arena;

#[test]
fn alloc_returns_mutable_reference() {
    let arena = Arena::new();
    let x: &mut u32 = arena.alloc(42);
    assert_eq!(*x, 42);
    *x = 100;
    assert_eq!(*x, 100);
}

#[test]
fn alloc_with_constructs_in_place() {
    let arena = Arena::new();
    let v: &mut std::vec::Vec<i32> = arena.alloc_with(|| vec![1, 2, 3]);
    v.push(4);
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn alloc_many_disjoint_mutable_refs_coexist() {
    // Simple references: multiple `&mut T` from the same arena are
    // disjoint, mutually-live mutable references.
    let arena = Arena::new();
    let a: &mut u64 = arena.alloc(1);
    let b: &mut u64 = arena.alloc(2);
    let c: &mut u64 = arena.alloc(3);
    *a += 10;
    *b += 20;
    *c += 30;
    assert_eq!(*a, 11);
    assert_eq!(*b, 22);
    assert_eq!(*c, 33);
}

#[test]
fn alloc_str_copies_and_returns_mut_str() {
    let arena = Arena::new();
    let s: &mut str = arena.alloc_str("hello");
    assert_eq!(s, "hello");
    s.make_ascii_uppercase();
    assert_eq!(s, "HELLO");
}

#[test]
fn alloc_str_empty() {
    let arena = Arena::new();
    let s: &mut str = arena.alloc_str("");
    assert_eq!(s, "");
}

#[test]
fn alloc_slice_copy_mutable() {
    let arena = Arena::new();
    let s: &mut [u32] = arena.alloc_slice_copy([1, 2, 3, 4, 5]);
    assert_eq!(s, &[1, 2, 3, 4, 5][..]);
    s[2] = 99;
    assert_eq!(s, &[1, 2, 99, 4, 5][..]);
}

#[test]
fn alloc_slice_clone_works() {
    let arena = Arena::new();
    let originals = [
        std::string::String::from("a"),
        std::string::String::from("b"),
        std::string::String::from("c"),
    ];
    let s: &mut [String] = arena.alloc_slice_clone(&originals);
    assert_eq!(s.len(), 3);
    assert_eq!(s[0], "a");
    s[0].push('!');
    assert_eq!(s[0], "a!");
    // Originals untouched.
    assert_eq!(originals[0], "a");
}

#[test]
fn alloc_slice_fill_with_works() {
    let arena = Arena::new();
    let s: &mut [u32] = arena.alloc_slice_fill_with(10, |i| (i as u32) * (i as u32));
    assert_eq!(s.len(), 10);
    for i in 0..10 {
        assert_eq!(s[i], (i as u32) * (i as u32));
    }
}

#[test]
fn try_alloc_slice_clone_works() {
    let arena = Arena::new();
    let originals = [std::string::String::from("x"), std::string::String::from("y")];
    let s: &mut [String] = arena.try_alloc_slice_clone(&originals).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0], "x");
    assert_eq!(s[1], "y");
}

#[test]
fn alloc_slice_fill_iter_works() {
    let arena = Arena::new();
    let s: &mut [u64] = arena.alloc_slice_fill_iter([0_u64, 1, 2, 3, 4]);
    assert_eq!(s, &[0, 1, 2, 3, 4]);
}

#[test]
fn try_alloc_slice_fill_iter_works() {
    let arena = Arena::new();
    let s: &mut [i32] = arena.try_alloc_slice_fill_iter([10, 20, 30]).unwrap();
    assert_eq!(s, &[10, 20, 30]);
}

#[test]
fn alloc_slice_fill_iter_empty() {
    let arena = Arena::new();
    let s: &mut [u32] = arena.alloc_slice_fill_iter(core::iter::empty::<u32>());
    assert!(s.is_empty());
}

#[test]
fn alloc_survives_chunk_rotation() {
    // Force chunk rotation while a bump-ref is alive. Without pinning,
    // the rotated chunk would be freed and the &mut would dangle.
    let arena = Arena::builder().build();
    let pinned_value: &mut [u8] = arena.alloc_slice_copy([0xAB; 1024]);
    pinned_value[0] = 0xCD;
    // Force chunk rotation: allocate enough to retire the current chunk.
    for _ in 0..10 {
        let _filler = arena.alloc_slice_copy([0_u8; 1024]);
    }
    // The original bump-ref must still be valid.
    assert_eq!(pinned_value[0], 0xCD);
    assert_eq!(pinned_value[1023], 0xAB);
    assert_eq!(pinned_value.len(), 1024);
}

#[test]
fn alloc_works_across_many_rotations() {
    // Stress test: many bump-allocs spanning many chunks. All
    // references must remain valid.
    let arena = Arena::builder().build();
    let mut refs: std::vec::Vec<&mut u32> = std::vec::Vec::with_capacity(1000);
    for i in 0..1000_u32 {
        refs.push(arena.alloc(i));
    }
    for (i, r) in refs.iter().enumerate() {
        assert_eq!(**r, i as u32);
    }
}

#[test]
fn alloc_drop_runs_at_arena_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter(std::sync::Arc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let _r1: &mut DropCounter = arena.alloc(DropCounter(std::sync::Arc::clone(&counter)));
        let _r2: &mut DropCounter = arena.alloc(DropCounter(std::sync::Arc::clone(&counter)));
        let _r3: &mut DropCounter = arena.alloc(DropCounter(std::sync::Arc::clone(&counter)));
        assert_eq!(counter.load(Ordering::SeqCst), 0, "drop must not run before arena drop");
        // arena drops here → all three DropCounters must run.
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn alloc_slice_fill_with_drop_runs_at_arena_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter(std::sync::Arc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let _slice: &mut [DropCounter] = arena.alloc_slice_fill_with(7, |_| DropCounter(std::sync::Arc::clone(&counter)));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 7);
}

#[test]
fn alloc_lifetime_bound_by_arena_borrow() {
    // Compile-time check: this compiles because the bump-ref's
    // lifetime is bounded by the arena reference.
    fn use_arena<'a>(arena: &'a Arena) -> &'a mut u32 {
        arena.alloc(7)
    }
    let arena = Arena::new();
    let r = use_arena(&arena);
    assert_eq!(*r, 7);
}

#[cfg(feature = "stats")]
#[test]
fn alloc_charges_stats() {
    let arena = Arena::new();
    let _r: &mut u64 = arena.alloc(42);
    // After any successful allocation, the provider must have obtained
    // at least one chunk from the underlying allocator; that chunk is
    // strictly larger than the 8-byte payload.
    assert!(arena.stats().total_bytes_allocated >= 8);
}

/// `wasted_tail_bytes` is a *live* gauge of the unused tail space
/// across the active `current_*` chunks plus any chunks that have
/// been retired from a `current_*` slot but not yet cached or
/// destroyed. It must be zero on a fresh arena (no chunks held at
/// all), and return to zero after `reset` releases every chunk back
/// to the cache / underlying allocator (which leaves the arena with
/// the empty-mutator sentinels, contributing 0 slack each).
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_bytes_is_live_and_returns_to_zero_after_reset() {
    let mut arena = Arena::new();
    assert_eq!(arena.stats().wasted_tail_bytes, 0, "fresh arena has no chunks");
    // Force at least one allocation so the arena obtains a chunk.
    for _ in 0..32 {
        let _r: &mut u64 = arena.alloc(0);
    }
    // The active chunk now contributes its free tail to the gauge
    // (the chunk has plenty of room left even after 32 u64 allocs).
    assert!(
        arena.stats().wasted_tail_bytes > 0,
        "active chunk's free tail must contribute to wasted_tail_bytes",
    );
    arena.reset();
    // After reset every chunk has been released (either cached or
    // destroyed). `current_*` are reset to empty-mutator sentinels
    // which contribute 0 slack.
    assert_eq!(
        arena.stats().wasted_tail_bytes,
        0,
        "reset returned every chunk and reinstalled the empty sentinel; \
         wasted-tail must be zero again",
    );
}

/// Specifically exercises the "active chunk contributes its tail"
/// path: with zero retired chunks, `wasted_tail_bytes` must still
/// reflect the free region of the current local/chunks, and
/// it must shrink as further allocations consume that region.
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_includes_active_chunks_and_shrinks_with_allocs() {
    let arena = Arena::new();
    // Trigger a single small local alloc to pin a chunk.
    let _: &mut u8 = arena.alloc(0);
    let chunks_before = arena.stats().normal_chunks_allocated;
    let after_one = arena.stats().wasted_tail_bytes;
    assert!(after_one > 0, "active chunk's free tail must be included even with 0 retires");
    // A handful of small allocs that fit in the current chunk's
    // remaining capacity. The free tail must strictly decrease
    // because no refill occurred.
    for _ in 0..16 {
        let _: &mut u64 = arena.alloc(0);
    }
    assert_eq!(
        arena.stats().normal_chunks_allocated,
        chunks_before,
        "test relies on no refill happening; tighten the loop if this fires",
    );
    let after_many = arena.stats().wasted_tail_bytes;
    assert!(
        after_many < after_one,
        "subsequent allocs consumed bump space; the active chunk's \
         contribution to wasted_tail_bytes must shrink (before={after_one}, \
         after={after_many})",
    );
}

/// Retiring a chunk while a smart-pointer handle still holds it alive
/// keeps the wasted tail counted until the handle drops (the chunk
/// reaches `release` only when its refcount finally hits zero).
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_bytes_is_held_by_outstanding_arc() {
    let arena = Arena::new();
    // Allocate one `Arc` in the initial (small) chunk and keep
    // it alive across a refill that retires the chunk.
    let pinned = arena.alloc_arc::<u64>(7);
    // Force the current chunk to refill until at least one
    // chunk gets retired without being released (i.e., a handle is
    // still keeping it alive). Detect retire via the chunk
    // count rather than the wasted-tail gauge, because the latter is
    // never zero now that the active chunk's free tail contributes.
    let initial_chunks = arena.stats().normal_chunks_allocated;
    let mut tries = 0;
    while arena.stats().normal_chunks_allocated == initial_chunks {
        // 2 KiB slice allocations fill the small initial chunk
        // quickly; refill is triggered well before we hit the cap.
        drop(arena.alloc_slice_copy_arc::<u8>(&[0_u8; 2048]));
        tries += 1;
        assert!(tries < 1_000, "chunk never refilled — retire path appears broken");
    }
    let held_wasted = arena.stats().wasted_tail_bytes;
    drop(pinned);
    // With the handle gone the original chunk's refcount hits zero,
    // it routes through `release`, and the counter decrements.
    let after_drop = arena.stats().wasted_tail_bytes;
    assert!(
        after_drop < held_wasted,
        "dropping the last handle must release the retired chunk's wasted tail \
         (before={held_wasted}, after={after_drop})",
    );
}

/// `refill_local` is the other major retire path: when the
/// current chunk is full, the old mutator is pushed
/// into `retired_local` (which keeps a `+1` for the duration of the
/// `&Arena` borrow). The wasted tail of every retired chunk must be
/// counted; `reset` releases every retired chunk back to the cache
/// and the counter must return cleanly to zero.
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_grows_on_local_refill_and_clears_on_reset() {
    let mut arena = Arena::new();
    // Force the first chunk to be acquired so subsequent allocs trigger
    // refills rather than the initial empty-mutator → first-chunk path.
    let _: &mut u8 = arena.alloc(0);
    let baseline = arena.stats().wasted_tail_bytes;
    let chunks_before = arena.stats().normal_chunks_allocated;
    let mut refills_observed = 0u64;
    let mut saw_growth_over_baseline = false;
    // Use a prime allocation size so the chunk cannot be exactly
    // exhausted (which would leave a true wasted-tail of zero); this
    // guarantees at least one refill leaves visible slack.
    //
    // `allocs` is a safety valve: if the chunk-allocation counter never
    // advances (e.g. a broken `stats()` / `normal_local()` / a
    // zero-length `alloc_slice_fill_with`), the refill condition can
    // never be met and this loop would otherwise spin forever. Bound it
    // so such a regression fails loudly instead of hanging.
    let mut allocs = 0u64;
    while refills_observed < 8 {
        let _: &mut [u8] = arena.alloc_slice_fill_with(509, |_| 0_u8);
        allocs += 1;
        assert!(
            allocs < 100_000,
            "after {allocs} allocations only {refills_observed}/8 refills were observed — \
             chunk-allocation accounting (stats/normal_local) or slice fill appears broken",
        );
        let now_chunks = arena.stats().normal_chunks_allocated;
        if now_chunks > chunks_before + refills_observed {
            refills_observed += 1;
            // After a refill the gauge must include both the retired
            // chunk's tail AND the new active chunk's tail, so it must
            // exceed the baseline (which was only the very first
            // active chunk's tail just after one tiny alloc).
            if arena.stats().wasted_tail_bytes > baseline {
                saw_growth_over_baseline = true;
            }
        }
    }
    assert!(
        saw_growth_over_baseline,
        "across {refills_observed} refills with a prime allocation size, \
         the wasted-tail counter never exceeded its single-active-chunk \
         baseline — retire-side accounting is broken",
    );
    arena.reset();
    assert_eq!(
        arena.stats().wasted_tail_bytes,
        0,
        "reset must release every chunk and reinstall the empty sentinels, \
         taking the gauge back to zero",
    );
}

/// **Conservation invariant**: across a full retire-and-release cycle,
/// the local wasted-tail counter must return to exactly its starting
/// value. Catches off-by-one or asymmetric-arithmetic bugs (e.g., add
/// 4 KiB, subtract 4096) that observation-of-non-zero tests would miss.
///
/// Only local allocation paths are exercised here: `reset` governs local
/// chunks, so it is what takes the gauge back to zero. Shared-chunk
/// wasted tail is released by handle drop plus chunk turnover (not by
/// `reset`) and is covered by the drop/cache-reuse tests above.
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_returns_to_exactly_baseline_across_full_cycle() {
    let mut arena = Arena::new();
    for cycle in 0..10 {
        let before = arena.stats().wasted_tail_bytes;
        assert_eq!(before, 0, "cycle {cycle}: baseline must be 0 before allocations begin");
        for _ in 0..4 {
            let _: &mut u64 = arena.alloc(42);
            let _: &mut [u8] = arena.alloc_slice_fill_with(256, |_| 0);
        }
        arena.reset();
        let after = arena.stats().wasted_tail_bytes;
        assert_eq!(
            after, 0,
            "cycle {cycle}: after reset, the local counter must return to exactly 0 \
             (got {after}) — asymmetric add/subtract would leave a residue",
        );
    }
}

/// Cache-reuse must not leak: a chunk that's cached on reset, then
/// re-acquired in the next epoch, then re-retired must contribute its
/// new wasted tail (not the stale stashed value from the previous
/// epoch, and not double-counted).
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_correct_after_cache_reuse_cycles() {
    let mut arena = Arena::new();
    let mut acquired_chunks_total = 0u64;
    for _ in 0..8 {
        // Force at least one full chunk's worth of allocs so we cycle
        // through `current` AND populate the cache on reset.
        for _ in 0..64 {
            let _: &mut u64 = arena.alloc(0);
        }
        let stats = arena.stats();
        acquired_chunks_total = stats.normal_chunks_allocated;
        arena.reset();
        // After every reset the counter must be 0 — even though the
        // chunk's `wasted_at_retire` field still holds the previous
        // value, the subtract at cache-push consumed it exactly once,
        // and the next epoch's retire will re-set + re-add.
        assert_eq!(arena.stats().wasted_tail_bytes, 0, "cache reuse leaked into wasted-tail counter");
    }
    // Sanity: we actually exercised real allocations, not a no-op.
    assert!(acquired_chunks_total >= 1, "test did not allocate any chunks");
}

/// Multiple chunks pinned by outstanding Arcs each contribute
/// their wasted tail; dropping the handles one at a time must
/// monotonically shrink the counter without underflow.
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_decreases_monotonically_as_pinned_arcs_drop() {
    let arena = Arena::new();
    let mut pins = std::vec::Vec::new();
    // Build up several pinned chunks by interleaving a pin with allocs
    // that force a refill. A few moderately sized copies per pin
    // overflow the (initially small) chunk, retiring it while the
    // pin holds it — far fewer allocations than a long inner loop.
    for _ in 0..4 {
        pins.push(arena.alloc_arc::<u64>(99));
        for _ in 0..3 {
            drop(arena.alloc_slice_copy_arc::<u8>(&[0_u8; 2048]));
        }
    }
    let peak = arena.stats().wasted_tail_bytes;
    // We may not get a contribution from every iteration (some Arcs
    // may share a chunk with later ones), but at least some chunks
    // were retired while pinned.
    assert!(peak > 0, "expected outstanding pins to keep retired chunks counted");
    // Drop the pins. The counter must never grow, never underflow,
    // and end at most equal to whatever the currently-active chunk
    // would contribute (which is 0 since it's not yet retired).
    let mut prev = peak;
    while let Some(p) = pins.pop() {
        drop(p);
        let cur = arena.stats().wasted_tail_bytes;
        assert!(cur <= prev, "dropping a pin must never grow the counter (prev={prev}, cur={cur})");
        // Underflow on a u64 atomic would show up as a value near
        // `u64::MAX`. Guard against that explicitly.
        assert!(
            cur < u64::MAX / 2,
            "counter underflowed (cur={cur}); subtract was unbalanced from add",
        );
        prev = cur;
    }
}

/// Oversized local allocations route through `alloc_oversized_local_*`,
/// which pushes a temporary mutator into `retired_local` so the
/// caller's simple reference can outlive the call. That mutator's
/// chunk participates in wasted-tail accounting just like a refill-
/// retired chunk: it must contribute on retire and release exactly on
/// reset.
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_handles_oversized_local_retire() {
    let mut arena = Arena::new();
    // Three oversized allocations create three retired oversized chunks.
    let _: &mut [u8] = arena.alloc_slice_fill_with(20 * 1024, |_| 0_u8);
    let mid = arena.stats().wasted_tail_bytes;
    let _: &mut [u8] = arena.alloc_slice_fill_with(20 * 1024, |_| 0_u8);
    let _: &mut [u8] = arena.alloc_slice_fill_with(20 * 1024, |_| 0_u8);
    let after = arena.stats().wasted_tail_bytes;
    // Each oversized chunk is sized to its request plus alignment and
    // drop-entry slack; the wasted tail per chunk may be 0 or small
    // depending on alignment. Either way, accumulating retires must
    // never *decrease* the counter (no spurious subtracts).
    assert!(
        after >= mid,
        "more oversized retires must not shrink the counter (mid={mid}, after={after})",
    );
    arena.reset();
    assert_eq!(
        arena.stats().wasted_tail_bytes,
        0,
        "reset must release every oversized-retired chunk",
    );
}

/// Smoke-test against u64 wrap-around: stress every retire+release path
/// many times. If the subtract ever exceeded the matching add even by
/// one byte, the running counter would underflow to a value near
/// `u64::MAX`.
///
/// The conservation bound is `wasted_tail_bytes <= total_bytes_allocated`:
/// the arena cannot waste more tail than it currently holds. This holds
/// regardless of whether the slack lives in local or (still-installed)
/// chunks, and an underflow would blow the wasted gauge far past
/// the total. `reset` only clears local wasted tail, so it is not
/// expected to drive the gauge to zero while a chunk is live.
#[cfg(feature = "stats")]
#[test]
fn wasted_tail_never_underflows_under_stress() {
    let mut arena = Arena::new();
    let filler = [0_u8; 64];
    for _ in 0..10 {
        let _: &mut u64 = arena.alloc(0);
        let _: &mut [u8] = arena.alloc_slice_copy(filler);
        drop(arena.alloc_arc::<u64>(0));
        drop(arena.alloc_box::<u64>(0));
        drop(arena.alloc_slice_copy_arc::<u8>(&[0_u8; 4096]));
        let stats = arena.stats();
        assert!(
            stats.wasted_tail_bytes <= stats.total_bytes_allocated,
            "wasted tail ({}) must never exceed total bytes outstanding ({}) — \
             an underflow would wrap it near u64::MAX",
            stats.wasted_tail_bytes,
            stats.total_bytes_allocated,
        );
    }
    arena.reset();
    let stats = arena.stats();
    assert!(stats.wasted_tail_bytes <= stats.total_bytes_allocated);
}

use crate::common::FailingAllocator;

#[test]
fn try_alloc_returns_err_on_failing_allocator() {
    let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
    assert!(arena.try_alloc(0_u32).is_err());
    assert!(arena.try_alloc_with(|| 0_u32).is_err());
    assert!(arena.try_alloc_slice_copy::<u8>(&[1, 2, 3]).is_err());
    assert!(arena.try_alloc_slice_fill_with::<u32, _>(3, |i| i as u32).is_err());
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_panics_on_failing_allocator() {
    let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
    let _ = arena.alloc(0_u32);
}

// Mixing with Allocator-trait usage (Vec<T, &Arena>) — pinning the same
// chunk both ways should still tear down cleanly.

#[test]
fn pinned_chunk_with_allocator_api2_vec_drops_cleanly() {
    let arena: Arena = Arena::builder().build();
    let _bump_ref: &mut u32 = arena.alloc(123);
    let mut v: allocator_api2::vec::Vec<u8, &Arena> = allocator_api2::vec::Vec::new_in(&arena);
    for _ in 0..5_000_u32 {
        v.push(0);
    }
    assert!(_bump_ref == &mut 123);
    drop(v);
    // arena drops at end-of-scope; pinned chunk is freed cleanly.
}

// Slack reclamation interaction with cache: cached chunk should NOT be
// pinned (cache reuse must reset the flag).
