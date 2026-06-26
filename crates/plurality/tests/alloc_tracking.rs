// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! System-allocation behaviour tests: prove that once a [`Pool`] has grown to
//! cover its working set, a steady state of allocate-then-free reuses the
//! pool's slots and touches the system allocator **zero** times.
//!
//! These use the `alloc_tracker` crate, whose `Allocator` global-allocator
//! wrapper counts every byte the process hands to the system allocator. Each
//! operation's thread-local span measures only the work inside it, so the
//! pool's chunk allocations are observed in isolation. The handles a pool hands
//! out (`Box`/`Arc`/`Rc`) are carved from the pool's own chunks, not the
//! system, so allocating them only touches the system when the pool grows a new
//! chunk. The vectors that hold the handles are pre-reserved outside the spans
//! so their growth is never counted.
//!
//! A pool grows chunks on demand and keeps them until it is dropped — freeing a
//! handle only returns its slot to the free list. So the invariant we assert is
//! the steady state: once the working set's chunks all exist, repeating the same
//! allocate/free workload reuses those chunks and never reallocates.

// Excluded under Miri: these measure real system-allocator traffic, which Miri
// (with its own allocator model) does not represent.
#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::collection_is_never_read, reason = "tests retain handles only to keep slots occupied")]

use alloc_tracker::{Allocator, Session};
use plurality::Pool;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Number of live values per fill — large enough to span several chunks at the
/// default chunk size.
const WORKLOAD: usize = 2_000;
/// Warm-up cycles, generous enough to grow every chunk the working set needs and
/// reach steady state before we measure.
const WARMUP_CYCLES: usize = 4;
/// Steady-state cycles to measure.
const STEADY_CYCLES: usize = 16;

/// A session that neither prints to stdout nor writes JSON to the target dir.
fn quiet_session() -> Session {
    Session::new().no_stdout().no_file()
}

/// The very first fill must grow chunks from the system, but once warmed, an
/// identical fill after dropping every handle reuses those chunks and touches
/// the system allocator zero times.
#[test]
fn first_fill_allocates_then_steady_state_is_zero() {
    // `Pool::new` allocates its inner state from the system; do it outside any
    // measured span.
    let pool = Pool::<u64>::new();
    let session = quiet_session();

    // Pre-reserve the holding vector outside any measured span so its growth
    // never pollutes the pool measurements.
    let mut hold: std::vec::Vec<plurality::Box<u64>> = std::vec::Vec::with_capacity(WORKLOAD);

    // The very first fill must obtain chunks from the system.
    let first = session.operation("first_fill");
    {
        let _span = first.measure_thread();
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_box(i as u64));
        }
    }
    assert!(
        first.total_bytes_allocated() > 0,
        "the first fill must grow chunk(s) from the system"
    );
    hold.clear();

    // Steady state: an identical fill after dropping all handles reuses slots.
    let reused = session.operation("refill");
    {
        let _span = reused.measure_thread();
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_box(i as u64));
        }
    }
    assert_eq!(
        reused.total_bytes_allocated(),
        0,
        "after warm-up, refilling must reuse the pool's chunks rather than reallocate"
    );
}

/// Steady-state `alloc_box`/free cycles must reuse slots: after warm-up, many
/// fill/drop cycles allocate nothing from the system.
#[test]
fn steady_state_box_fill_and_drop_does_not_allocate() {
    let pool = Pool::<u64>::new();
    let session = quiet_session();
    let mut hold: std::vec::Vec<plurality::Box<u64>> = std::vec::Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_box(i as u64));
        }
        hold.clear();
    }

    let steady = session.operation("box_steady_state");
    {
        let _span = steady.measure_thread();
        for _ in 0..STEADY_CYCLES {
            for i in 0..WORKLOAD {
                hold.push(pool.alloc_box(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        steady.total_bytes_allocated(),
        0,
        "steady-state Box fill/drop cycles must reuse slots, not allocate from the system"
    );
}

/// Steady-state `alloc_arc`/free cycles must reuse slots (the atomic-refcount
/// shared handle path).
#[test]
fn steady_state_arc_fill_and_drop_does_not_allocate() {
    let pool = Pool::<u64>::new();
    let session = quiet_session();
    let mut hold: std::vec::Vec<plurality::Arc<u64>> = std::vec::Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_arc(i as u64));
        }
        hold.clear();
    }

    let steady = session.operation("arc_steady_state");
    {
        let _span = steady.measure_thread();
        for _ in 0..STEADY_CYCLES {
            for i in 0..WORKLOAD {
                hold.push(pool.alloc_arc(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        steady.total_bytes_allocated(),
        0,
        "steady-state Arc fill/drop cycles must reuse slots, not allocate from the system"
    );
}

/// Steady-state `alloc_rc`/free cycles must reuse slots (the non-atomic
/// single-threaded shared handle path).
#[test]
fn steady_state_rc_fill_and_drop_does_not_allocate() {
    let pool = Pool::<u64>::new();
    let session = quiet_session();
    let mut hold: std::vec::Vec<plurality::Rc<u64>> = std::vec::Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_rc(i as u64));
        }
        hold.clear();
    }

    let steady = session.operation("rc_steady_state");
    {
        let _span = steady.measure_thread();
        for _ in 0..STEADY_CYCLES {
            for i in 0..WORKLOAD {
                hold.push(pool.alloc_rc(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        steady.total_bytes_allocated(),
        0,
        "steady-state Rc fill/drop cycles must reuse slots, not allocate from the system"
    );
}

/// A rolling working set (free one slot, then allocate a replacement) models
/// real churn. After warm-up the freed slot is reused for the replacement, so
/// the system allocator is never touched.
#[test]
fn steady_state_rolling_churn_does_not_allocate() {
    let pool = Pool::<u64>::new();
    let session = quiet_session();

    // Keep a fixed-size working set live; `None` slots are replaced in place so
    // each replacement reuses the slot freed one statement earlier.
    let mut hold: std::vec::Vec<Option<plurality::Box<u64>>> = std::vec::Vec::with_capacity(WORKLOAD);
    for i in 0..WORKLOAD {
        hold.push(Some(pool.alloc_box(i as u64)));
    }

    // Warm up the churn pattern so every chunk it needs already exists.
    for _ in 0..WARMUP_CYCLES {
        for (i, slot) in hold.iter_mut().enumerate() {
            *slot = None; // free the slot first...
            *slot = Some(pool.alloc_box(i as u64)); // ...so the replacement reuses it.
        }
    }

    let steady = session.operation("rolling_churn");
    {
        let _span = steady.measure_thread();
        for _ in 0..STEADY_CYCLES {
            for (i, slot) in hold.iter_mut().enumerate() {
                *slot = None;
                *slot = Some(pool.alloc_box(i as u64));
            }
        }
    }
    assert_eq!(
        steady.total_bytes_allocated(),
        0,
        "steady-state rolling churn must reuse the just-freed slot, not allocate from the system"
    );
}
