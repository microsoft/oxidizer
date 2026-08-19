// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies with `alloc_tracker` that a warmed pool reuses slots without system
//! allocations. Holding vectors are reserved outside measured spans.

// Excluded under Miri: these measure real system-allocator traffic, which Miri
// (with its own allocator model) does not represent.
#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::collection_is_never_read, reason = "tests retain handles only to keep slots occupied")]

use std::alloc::System;

use alloc_tracker::{Allocator, Session};
use plurality::{Arc as PoolArc, Box as PoolBox, MultiPool, Pool, Rc as PoolRc};

/// Single-operation bodies for the pooled fat-pointer comparison. Kept as a
/// self-contained copy (rather than an include shared with the benches) so this
/// test pulls in no cross-target files.
mod dyn_box_ops {
    use std::boxed::Box as StdBox;
    use std::hint::black_box;

    use infinity_pool::{BlindPool, LocalBlindPool, LocalPinnedPool, PinnedPool, define_pooled_dyn_cast};
    use plurality::{Box as PoolBox, MultiPool, Pool, coerce};

    /// Number of reusable slots provisioned before measurement.
    pub(crate) const CAP: usize = 1024;

    #[derive(Clone)]
    pub(crate) struct Obj {
        tag: u64,
        payload: [u64; 3],
    }

    impl Obj {
        #[inline]
        fn new(i: u64) -> Self {
            Self {
                tag: i,
                payload: [i, i ^ 0xFF, i.wrapping_mul(0x9E37_79B9)],
            }
        }
    }

    pub(crate) trait Marker {
        fn tag(&self) -> u64;
    }

    impl Marker for Obj {
        #[inline]
        fn tag(&self) -> u64 {
            self.tag ^ self.payload[1]
        }
    }

    define_pooled_dyn_cast!(Marker);

    #[inline]
    fn invoke_dyn(value: &dyn Marker) {
        black_box(black_box(value).tag());
    }

    pub(crate) fn setup_plurality(n: usize) -> Pool<Obj> {
        let pool = Pool::<Obj>::new();
        let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
        drop(warm);
        assert!(pool.capacity() >= n as u64);
        assert!(pool.is_empty());
        let handle = pool.alloc_box(Obj::new(n as u64));
        let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
        assert_eq!(handle.tag(), 0xFF);
        drop(handle);
        pool
    }

    pub(crate) fn setup_plurality_multi(n: usize) -> MultiPool {
        let pool = MultiPool::new();
        let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
        drop(warm);
        assert!(pool.capacity_of::<Obj>() >= n as u64);
        assert!(pool.is_empty());
        let handle = pool.alloc_box(Obj::new(n as u64));
        let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
        assert_eq!(handle.tag(), 0xFF);
        drop(handle);
        pool
    }

    pub(crate) fn setup_infinity_pinned(n: usize) -> PinnedPool<Obj> {
        let pool = PinnedPool::new();
        pool.reserve(n);
        let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
        drop(warm);
        assert!(pool.capacity() >= n);
        assert!(pool.is_empty());
        let handle = pool.insert(Obj::new(n as u64)).cast_marker();
        assert_eq!(handle.tag(), 0xFF);
        drop(handle);
        pool
    }

    pub(crate) fn setup_infinity_local_pinned(n: usize) -> LocalPinnedPool<Obj> {
        let pool = LocalPinnedPool::new();
        pool.reserve(n);
        let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
        drop(warm);
        assert!(pool.capacity() >= n);
        assert!(pool.is_empty());
        let handle = pool.insert(Obj::new(n as u64)).cast_marker();
        assert_eq!(handle.tag(), 0xFF);
        drop(handle);
        pool
    }

    pub(crate) fn setup_infinity_blind(n: usize) -> BlindPool {
        let pool = BlindPool::new();
        pool.reserve_for::<Obj>(n);
        let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
        drop(warm);
        assert!(pool.capacity_for::<Obj>() >= n);
        assert!(pool.is_empty());
        let handle = pool.insert(Obj::new(n as u64)).cast_marker();
        assert_eq!(handle.tag(), 0xFF);
        drop(handle);
        pool
    }

    pub(crate) fn setup_infinity_local_blind(n: usize) -> LocalBlindPool {
        let pool = LocalBlindPool::new();
        pool.reserve_for::<Obj>(n);
        let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
        drop(warm);
        assert!(pool.capacity_for::<Obj>() >= n);
        assert!(pool.is_empty());
        let handle = pool.insert(Obj::new(n as u64)).cast_marker();
        assert_eq!(handle.tag(), 0xFF);
        drop(handle);
        pool
    }

    pub(crate) fn setup_std_box(n: usize) {
        let warm: Vec<StdBox<dyn Marker>> = (0..n).map(|i| StdBox::new(Obj::new(i as u64)) as StdBox<dyn Marker>).collect();
        black_box(&warm);
        drop(warm);
        let handle: StdBox<dyn Marker> = StdBox::new(Obj::new(n as u64));
        assert_eq!(black_box::<&dyn Marker>(&*handle).tag(), 0xFF);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn plurality_box(pool: &Pool<Obj>, i: u64) {
        let handle = pool.alloc_box(black_box(Obj::new(i)));
        let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn plurality_multi_box(pool: &MultiPool, i: u64) {
        let handle = pool.alloc_box(black_box(Obj::new(i)));
        let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn infinity_pinned(pool: &PinnedPool<Obj>, i: u64) {
        let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn infinity_local_pinned(pool: &LocalPinnedPool<Obj>, i: u64) {
        let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn infinity_blind(pool: &BlindPool, i: u64) {
        let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn infinity_local_blind(pool: &LocalBlindPool, i: u64) {
        let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }

    #[inline]
    pub(crate) fn std_box(i: u64) {
        let handle: StdBox<dyn Marker> = StdBox::new(black_box(Obj::new(i)));
        invoke_dyn(&*handle);
        drop(black_box(handle));
    }
}

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

/// Number of live values per fill — large enough to span several chunks at the
/// default chunk size.
const WORKLOAD: usize = 2_000;
/// Warm-up cycles needed to reach a steady state before measurement.
const WARMUP_CYCLES: usize = 4;
/// Steady-state cycles to measure.
const STEADY_CYCLES: usize = 16;

/// A session that neither prints to stdout nor writes JSON to the target dir.
fn quiet_session() -> Session {
    Session::new().no_stdout().no_file()
}

fn total_bytes_allocated(session: &Session, operation_name: &str) -> u64 {
    let missing_operation = format!("operation {operation_name:?} must match a name registered with Session::operation on this session");

    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == operation_name).then(|| operation.total_bytes_allocated()))
        .expect(&missing_operation)
}

fn assert_no_system_allocations(session: &Session, name: &str, mut f: impl FnMut()) {
    let operation = session.operation(name);
    {
        let _span = operation.measure_thread().iterations(dyn_box_ops::CAP as u64);
        for _ in 0..dyn_box_ops::CAP {
            f();
        }
    }
    assert_eq!(total_bytes_allocated(session, name), 0, "{name} must reuse pre-warmed storage");
}

#[test]
fn dyn_box_benchmark_allocation_behavior_matches_design() {
    let plurality = dyn_box_ops::setup_plurality(dyn_box_ops::CAP);
    let plurality_multi = dyn_box_ops::setup_plurality_multi(dyn_box_ops::CAP);
    let infinity = dyn_box_ops::setup_infinity_pinned(dyn_box_ops::CAP);
    let infinity_local = dyn_box_ops::setup_infinity_local_pinned(dyn_box_ops::CAP);
    let infinity_blind = dyn_box_ops::setup_infinity_blind(dyn_box_ops::CAP);
    let infinity_local_blind = dyn_box_ops::setup_infinity_local_blind(dyn_box_ops::CAP);
    let session = quiet_session();

    assert_no_system_allocations(&session, "plurality_dyn_box", || {
        dyn_box_ops::plurality_box(&plurality, 0);
    });
    assert_no_system_allocations(&session, "plurality_multi_dyn_box", || {
        dyn_box_ops::plurality_multi_box(&plurality_multi, 0);
    });
    assert_no_system_allocations(&session, "infinity_pinned_dyn_box", || {
        dyn_box_ops::infinity_pinned(&infinity, 0);
    });
    assert_no_system_allocations(&session, "infinity_local_pinned_dyn_box", || {
        dyn_box_ops::infinity_local_pinned(&infinity_local, 0);
    });
    assert_no_system_allocations(&session, "infinity_blind_dyn_box", || {
        dyn_box_ops::infinity_blind(&infinity_blind, 0);
    });
    assert_no_system_allocations(&session, "infinity_local_blind_dyn_box", || {
        dyn_box_ops::infinity_local_blind(&infinity_local_blind, 0);
    });

    dyn_box_ops::setup_std_box(dyn_box_ops::CAP);
    let heap = session.operation("std_dyn_box");
    {
        let _span = heap.measure_thread().iterations(1);
        dyn_box_ops::std_box(0);
    }
    assert!(
        total_bytes_allocated(&session, "std_dyn_box") > 0,
        "std::Box must include its per-operation heap allocation"
    );
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
    let mut hold: Vec<PoolBox<u64>> = Vec::with_capacity(WORKLOAD);

    // The very first fill must obtain chunks from the system.
    let first = session.operation("first_fill");
    {
        let _span = first.measure_thread().iterations(WORKLOAD as u64);
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_box(i as u64));
        }
    }
    assert!(
        total_bytes_allocated(&session, "first_fill") > 0,
        "the first fill must grow chunk(s) from the system"
    );
    hold.clear();

    // Steady state: an identical fill after dropping all handles reuses slots.
    let reused = session.operation("refill");
    {
        let _span = reused.measure_thread().iterations(WORKLOAD as u64);
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_box(i as u64));
        }
    }
    assert_eq!(
        total_bytes_allocated(&session, "refill"),
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
    let mut hold: Vec<PoolBox<u64>> = Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_box(i as u64));
        }
        hold.clear();
    }

    let steady = session.operation("box_steady_state");
    {
        let _span = steady.measure_thread().iterations((STEADY_CYCLES * WORKLOAD) as u64);
        for _ in 0..STEADY_CYCLES {
            for i in 0..WORKLOAD {
                hold.push(pool.alloc_box(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        total_bytes_allocated(&session, "box_steady_state"),
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
    let mut hold: Vec<PoolArc<u64>> = Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_arc(i as u64));
        }
        hold.clear();
    }

    let steady = session.operation("arc_steady_state");
    {
        let _span = steady.measure_thread().iterations((STEADY_CYCLES * WORKLOAD) as u64);
        for _ in 0..STEADY_CYCLES {
            for i in 0..WORKLOAD {
                hold.push(pool.alloc_arc(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        total_bytes_allocated(&session, "arc_steady_state"),
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
    let mut hold: Vec<PoolRc<u64>> = Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        for i in 0..WORKLOAD {
            hold.push(pool.alloc_rc(i as u64));
        }
        hold.clear();
    }

    let steady = session.operation("rc_steady_state");
    {
        let _span = steady.measure_thread().iterations((STEADY_CYCLES * WORKLOAD) as u64);
        for _ in 0..STEADY_CYCLES {
            for i in 0..WORKLOAD {
                hold.push(pool.alloc_rc(i as u64));
            }
            hold.clear();
        }
    }
    assert_eq!(
        total_bytes_allocated(&session, "rc_steady_state"),
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
    let mut hold: Vec<Option<PoolBox<u64>>> = Vec::with_capacity(WORKLOAD);
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
        let _span = steady.measure_thread().iterations((STEADY_CYCLES * WORKLOAD) as u64);
        for _ in 0..STEADY_CYCLES {
            for (i, slot) in hold.iter_mut().enumerate() {
                *slot = None;
                *slot = Some(pool.alloc_box(i as u64));
            }
        }
    }
    assert_eq!(
        total_bytes_allocated(&session, "rolling_churn"),
        0,
        "steady-state rolling churn must reuse the just-freed slot, not allocate from the system"
    );
}

/// One value of each layout the multi-pool workload presents. Holding them in a
/// tuple keeps every layout live simultaneously and makes each cycle alternate
/// between layout pools, so every allocation runs a directory scan.
type MixedRow = (PoolBox<u8>, PoolBox<u32>, PoolBox<u64>, PoolBox<[u64; 4]>);

fn fill_mixed(pool: &MultiPool, hold: &mut Vec<MixedRow>) {
    for i in 0..WORKLOAD {
        let wide = i as u64;
        hold.push((
            u8::try_from(i % 256).map(|v| pool.alloc_box(v)).unwrap(),
            u32::try_from(i).map(|v| pool.alloc_box(v)).unwrap(),
            pool.alloc_box(wide),
            pool.alloc_box([wide; 4]),
        ));
    }
}

/// A warmed multi pool must reuse slots exactly as a typed pool does, including
/// when the workload spans several layouts.
#[test]
fn steady_state_multi_mix_does_not_allocate() {
    let pool = MultiPool::new();
    let session = quiet_session();
    let mut hold: Vec<MixedRow> = Vec::with_capacity(WORKLOAD);

    for _ in 0..WARMUP_CYCLES {
        fill_mixed(&pool, &mut hold);
        hold.clear();
    }

    let steady = session.operation("multi_mix_steady_state");
    {
        let _span = steady.measure_thread().iterations((STEADY_CYCLES * WORKLOAD) as u64);
        for _ in 0..STEADY_CYCLES {
            fill_mixed(&pool, &mut hold);
            hold.clear();
        }
    }
    assert_eq!(
        total_bytes_allocated(&session, "multi_mix_steady_state"),
        0,
        "steady-state multi-pool fill/drop cycles must reuse slots across every layout, not allocate from the system"
    );
}
