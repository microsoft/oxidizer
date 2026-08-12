// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::unwrap_used,
    clippy::items_after_statements,
    reason = "test code"
)]

//! `Pool<T, A>: Send` does not require `T: Send`.
//!
//! A pool object owns no values, exposes no iteration or drain, and tears down
//! without running value destructors. A thread that receives a pool therefore
//! has no route to a value another thread placed in it. These tests hold that
//! reasoning to account by moving pools of a deliberately thread-bound type
//! across thread boundaries and driving every scenario the removed bound would
//! otherwise have forbidden.
//!
//! Run under Miri: the interesting failures are aliasing and data-race
//! violations, which a native run will not surface.

use std::cell::Cell;
use std::rc::Rc as StdRc;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::ThreadId;

use plurality::Pool;

/// Moves anything across a thread boundary, including the values that make the
/// scenarios below thread-bound.
///
/// The pool itself does not need this — it is `Send` regardless of `T` — but
/// the closure passed to a scoped thread captures the `StdRc` handles and drop
/// observers alongside it, and those are genuinely `!Send`.
struct AssertSend<T>(T);

impl<T> AssertSend<T> {
    /// Takes `self` by value so closures capture the whole shim rather than
    /// reaching through it to the non-`Send` field.
    fn into_inner(self) -> T {
        self.0
    }
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "asserting `Send` for a non-`Send` payload is the entire purpose of this shim"
)]
// SAFETY: the enclosing tests establish, per scenario, that the wrapped value
// is never touched from two threads at once. This shim only defeats the
// compiler's inability to see that argument; it does not weaken it.
unsafe impl<T> Send for AssertSend<T> {}

/// Counts destructor runs for one test, so a test can assert that every value
/// it created was in fact destroyed.
type DropLog = StdArc<AtomicUsize>;

/// Non-`Send`, non-`Sync`: contains a `Cell` and an `StdRc`, so any
/// cross-thread access to the value itself would be a data race.
///
/// Each instance records the thread that built it, and its destructor asserts
/// that it is still on that thread. That assertion is the load-bearing part of
/// these tests: it turns "the far thread cannot reach this value" from a claim
/// into something the run actually checks.
#[derive(Debug)]
struct ThreadBound {
    counter: Cell<u64>,
    shared: StdRc<Cell<u64>>,
    created_on: ThreadId,
    drops: DropLog,
}

impl ThreadBound {
    fn new(shared: StdRc<Cell<u64>>, drops: &DropLog) -> Self {
        Self {
            counter: Cell::new(0),
            shared,
            created_on: std::thread::current().id(),
            drops: StdArc::clone(drops),
        }
    }

    fn bump(&self) {
        self.counter.set(self.counter.get() + 1);
        self.shared.set(self.shared.get() + 1);
    }
}

impl Drop for ThreadBound {
    fn drop(&mut self) {
        assert_eq!(
            std::thread::current().id(),
            self.created_on,
            "a thread-bound value was destroyed on a thread other than the one that created it"
        );
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

/// Pins the bound itself: `Pool<T, A>: Send` must not depend on `T`.
///
/// `ThreadBound` is neither `Send` nor `Sync`, so this fails to compile the
/// moment a `T: Send` bound is reintroduced on the `Send` impl.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Pool<ThreadBound>>();
};

/// The pool object moves to another thread while a handle to a non-`Send`
/// value stays behind on the originating thread.
///
/// This is the scenario a `T: Send` bound would forbid.
#[test]
fn pool_moves_while_non_send_value_stays_behind() {
    let shared = StdRc::new(Cell::new(0_u64));
    let pool: Pool<ThreadBound> = Pool::builder().build();
    let origin_drops = DropLog::default();
    let far_drops = DropLog::default();

    let retained = pool.alloc_box(ThreadBound::new(StdRc::clone(&shared), &origin_drops));
    retained.bump();

    // The pool crosses the thread boundary; the value does not.
    let moved = AssertSend((pool, StdArc::clone(&far_drops)));
    let handle = std::thread::spawn(move || {
        let (pool, far_drops) = moved.into_inner();

        // The receiving thread allocates and frees freely. It never reaches
        // the value the originating thread still holds, because the pool
        // offers no iteration.
        let mut kept = Vec::new();
        for _ in 0_i32..64_i32 {
            kept.push(pool.alloc_box(ThreadBound::new(StdRc::new(Cell::new(0)), &far_drops)));
        }
        for value in &kept {
            value.bump();
        }
        drop(kept);

        // Drop the pool object here, on the receiving thread.
        drop(pool);
    });

    // Meanwhile the originating thread keeps using its own value.
    retained.bump();
    handle.join().unwrap();

    // Every value the far thread built was destroyed there.
    assert_eq!(far_drops.load(Ordering::Relaxed), 64);

    // The retained handle outlived the pool object and still works.
    retained.bump();
    assert_eq!(retained.counter.get(), 3);
    assert_eq!(shared.get(), 3);

    drop(retained);
    assert_eq!(shared.get(), 3);
    assert_eq!(origin_drops.load(Ordering::Relaxed), 1);
}

/// Slot reuse across the thread boundary: the originating thread frees a slot,
/// then the receiving thread allocates and gets that same memory back.
///
/// This is the case where the two threads genuinely touch the same bytes.
#[test]
fn slot_reuse_across_threads_with_non_send_values() {
    let pool: Pool<ThreadBound> = Pool::builder().build();
    let shared = StdRc::new(Cell::new(0_u64));
    let origin_drops = DropLog::default();
    let far_drops = DropLog::default();

    // Occupy and release several slots so the free list has entries that
    // previously held values constructed on this thread.
    let mut first = Vec::new();
    for _ in 0_i32..16_i32 {
        first.push(pool.alloc_box(ThreadBound::new(StdRc::clone(&shared), &origin_drops)));
    }
    for value in &first {
        value.bump();
    }
    let before = shared.get();
    drop(first);
    assert_eq!(origin_drops.load(Ordering::Relaxed), 16);

    let moved = AssertSend((pool, StdArc::clone(&far_drops)));
    let handle = std::thread::spawn(move || {
        let (pool, far_drops) = moved.into_inner();

        // These allocations reuse the slots freed above.
        let mut second = Vec::new();
        for _ in 0_i32..16_i32 {
            second.push(pool.alloc_box(ThreadBound::new(StdRc::new(Cell::new(0)), &far_drops)));
        }
        for value in &second {
            value.bump();
        }
        drop(second);
        AssertSend(pool)
    });

    let pool = handle.join().unwrap().into_inner();

    // The far thread destroyed exactly its own values, in slots this thread
    // had previously filled and released.
    assert_eq!(far_drops.load(Ordering::Relaxed), 16);

    // The originating thread's `Rc` was never touched by the other thread.
    assert_eq!(shared.get(), before);
    assert_eq!(StdRc::strong_count(&shared), 1);
    drop(pool);
}

/// The pool object is dropped on the receiving thread while a handle is still
/// live, so teardown is deferred and ultimately runs on the *originating*
/// thread when that last handle departs.
#[test]
fn teardown_on_far_thread_with_non_send_values() {
    let pool: Pool<ThreadBound> = Pool::builder().build();
    let shared = StdRc::new(Cell::new(0_u64));
    let drops = DropLog::default();

    let retained = pool.alloc_box(ThreadBound::new(StdRc::clone(&shared), &drops));

    let moved = AssertSend(pool);
    let handle = std::thread::spawn(move || {
        let pool = moved.into_inner();
        // Drop the pool object; the retained handle still holds the memory.
        drop(pool);
    });
    handle.join().unwrap();

    // The pool object is gone. Teardown happens here, on the original thread,
    // when this last handle departs.
    retained.bump();
    assert_eq!(shared.get(), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(retained);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

/// The reverse: the last handle departs on the *receiving* thread, so teardown
/// runs there. The value itself is `Send` here, since the handle must cross;
/// what is being probed is that teardown touches no value state.
#[test]
fn teardown_on_far_thread_via_last_handle() {
    let pool: Pool<u64> = Pool::builder().build();

    let retained = pool.alloc_box(7_u64);
    drop(pool);

    let moved = AssertSend(retained);
    let handle = std::thread::spawn(move || {
        let retained = moved.into_inner();
        assert_eq!(*retained, 7);
        // Teardown of the whole pool runs here.
        drop(retained);
    });
    handle.join().unwrap();
}

/// The strongest case: the pool moves to a thread that allocates from it, while
/// the originating thread concurrently drops handles to non-`Send` values into
/// the same free list.
///
/// This exercises the single-producer / multi-consumer hand-off with values
/// whose types would forbid the pool from moving at all under a `T: Send`
/// bound. Allocation stays on one thread throughout — it simply is not the
/// thread that built the pool.
///
/// A caveat on what this proves. A barrier releases both threads together, but
/// nothing forces a free push to overlap a pop, and Miri explores the schedule
/// it happens to run rather than all of them. So a pass is evidence that the
/// interleavings actually reached are clean, not a proof that every
/// interleaving is. The compile-time assertion above and the per-value thread
/// affinity check in `ThreadBound::drop` are the parts that hold regardless of
/// scheduling; this test adds pressure on the hand-off, nothing stronger.
#[test]
fn concurrent_free_and_alloc_with_non_send_values() {
    let shared = StdRc::new(Cell::new(0_u64));
    let pool: Pool<ThreadBound> = Pool::builder().build();
    let origin_drops = DropLog::default();
    let far_drops = DropLog::default();

    // Build handles on this thread. They are not `Send`, so they stay here.
    let mut retained = Vec::new();
    for _ in 0_i32..256_i32 {
        retained.push(pool.alloc_box(ThreadBound::new(StdRc::clone(&shared), &origin_drops)));
    }
    for value in &retained {
        value.bump();
    }
    let expected = shared.get();

    let barrier = std::sync::Barrier::new(2);
    let barrier = &barrier;
    let far_drops_moved = StdArc::clone(&far_drops);
    std::thread::scope(|scope| {
        // The pool crosses the boundary by value, which is the relaxation under
        // test: `ThreadBound` is `!Send`, and the pool moves anyway. Sharing a
        // `&Pool` instead would assert `Sync`, which the crate withholds.
        scope.spawn(move || {
            barrier.wait();
            // Allocate and free continuously on the far thread, popping slots
            // the originating thread is concurrently pushing.
            for _ in 0_i32..256_i32 {
                let owned =
                    pool.alloc_box(ThreadBound::new(StdRc::new(Cell::new(0)), &far_drops_moved));
                owned.bump();
            }
        });

        barrier.wait();
        // Concurrently release the originating thread's handles.
        for value in retained {
            drop(value);
        }
    });

    // Each thread destroyed exactly the values it created, on its own thread.
    assert_eq!(origin_drops.load(Ordering::Relaxed), 256);
    assert_eq!(far_drops.load(Ordering::Relaxed), 256);

    // The originating thread's `Rc` was never observed by the far thread.
    assert_eq!(shared.get(), expected);
    assert_eq!(StdRc::strong_count(&shared), 1);
}
