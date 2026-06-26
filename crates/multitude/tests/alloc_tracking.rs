// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! System-allocation behaviour tests: prove the [`Arena`] reuses chunk memory
//! and does not over-allocate from the system allocator around
//! [`Arena::reset`] and around all `Arc`s being dropped.
//!
//! These use the `alloc_tracker` crate, whose `Allocator` global-allocator
//! wrapper counts every byte the process hands to the system allocator. Each
//! operation's thread-local span measures only the work inside it, so the
//! `Arena`'s chunk allocations are observed in isolation (the holding vectors
//! are pre-reserved outside the spans so their growth is never counted).
//!
//! The arena ratchets its chunk size class upward during a warm-up phase, so
//! the invariant we assert is the steady state: once warmed, repeating the
//! exact same workload reuses the arena's chunks and touches the system
//! allocator **zero** times.

// Excluded under Miri: these measure real system-allocator traffic, which Miri
// (with its own allocator model) does not represent.
#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::collection_is_never_read, reason = "tests retain Arc handles only to keep chunks alive")]

use alloc_tracker::{Allocator, Session};
use multitude::{Arc, Arena};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Number of values per fill — large enough to span several chunks.
const WORKLOAD: usize = 2_000;
/// Warm-up cycles, generous enough to let the chunk size class fully ratchet
/// and the cache reach steady state before we measure.
const WARMUP_CYCLES: usize = 16;

/// A session that neither prints to stdout nor writes JSON to the target dir.
fn quiet_session() -> Session {
    Session::new().no_stdout().no_file()
}

/// After [`Arena::reset`], re-allocating the same workload reuses the arena's
/// chunks: the first fill allocates from the system, but once warmed, a reset
/// + identical refill touches the system allocator zero times.
#[test]
fn reset_reuses_chunks_without_reallocating() {
    let mut arena = Arena::new();
    let session = quiet_session();

    // The very first fill must obtain chunks from the system.
    let first = session.operation("first_fill");
    {
        let _span = first.measure_thread();
        for i in 0..WORKLOAD {
            let _v = arena.alloc(i as u64);
        }
    }
    assert!(
        first.total_bytes_allocated() > 0,
        "first fill must allocate chunk(s) from the system"
    );
    arena.reset();

    // Warm up: repeated fill+reset cycles let the size class settle.
    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            let _v = arena.alloc(i as u64);
        }
        arena.reset();
    }

    // Steady state: an identical fill after reset reuses the existing chunks.
    let reused = session.operation("refill_after_reset");
    {
        let _span = reused.measure_thread();
        for i in 0..WORKLOAD {
            let _v = arena.alloc(i as u64);
        }
    }
    assert_eq!(
        reused.total_bytes_allocated(),
        0,
        "after warm-up, reset must reuse chunks rather than reallocate from the system"
    );
}

/// Dropping every `Arc` lets the arena reclaim those chunks early. Once warmed,
/// re-allocating the same workload reuses the reclaimed chunks and touches the
/// system allocator zero times.
#[test]
fn dropping_all_arcs_reclaims_chunks_for_reuse() {
    let arena = Arena::new();
    let session = quiet_session();

    // Pre-reserve the holding vector *outside* any measured span so its growth
    // never pollutes the arena measurements.
    let mut hold: std::vec::Vec<Arc<u64>> = std::vec::Vec::with_capacity(WORKLOAD);

    // The very first fill must obtain chunks from the system.
    let first = session.operation("arc_first_fill");
    {
        let _span = first.measure_thread();
        for i in 0..WORKLOAD {
            hold.push(arena.alloc_arc(i as u64));
        }
    }
    assert!(
        first.total_bytes_allocated() > 0,
        "allocating arcs must allocate chunk(s) from the system"
    );
    // Dropping every Arc drives each chunk's strong count to zero, reclaiming
    // it into the provider's cache (early reclamation) rather than freeing it.
    hold.clear();

    // Warm up: fill/drop cycles let the size class and cache settle.
    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(arena.alloc_arc(i as u64));
        }
        hold.clear();
    }

    // Steady state: an identical fill after dropping all arcs reuses chunks.
    let reused = session.operation("arc_refill");
    {
        let _span = reused.measure_thread();
        for i in 0..WORKLOAD {
            hold.push(arena.alloc_arc(i as u64));
        }
    }
    assert_eq!(
        reused.total_bytes_allocated(),
        0,
        "after warm-up, re-allocating dropped arcs must reuse reclaimed chunks"
    );
}

/// A steady state of allocate-then-drop must not grow unboundedly: after
/// warm-up, many fill/drop cycles reuse the same chunks, so the system
/// allocator is touched zero times across all of them.
#[test]
fn steady_state_fill_and_drop_does_not_over_allocate() {
    let arena = Arena::new();
    let session = quiet_session();
    let mut hold: std::vec::Vec<Arc<u64>> = std::vec::Vec::with_capacity(WORKLOAD);

    // Warm up so the working set's chunks all exist and are cached.
    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(arena.alloc_arc(i as u64));
        }
        hold.clear();
    }

    // Steady state: many more cycles must allocate nothing from the system.
    let steady = session.operation("steady_state");
    {
        let _span = steady.measure_thread();
        for _round in 0..16 {
            for i in 0..WORKLOAD {
                hold.push(arena.alloc_arc(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        steady.total_bytes_allocated(),
        0,
        "steady-state fill/drop cycles must reuse chunks, not over-allocate from the system"
    );
}
