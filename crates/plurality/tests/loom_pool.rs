// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::clone_on_ref_ptr,
    clippy::unwrap_used,
    clippy::assertions_on_result_states,
    clippy::cast_possible_truncation,
    clippy::collection_is_never_read,
    clippy::items_after_statements,
    clippy::many_single_char_names,
    clippy::borrow_as_ptr,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    reason = "test and benchmark code"
)]

//! Loom concurrency-permutation tests. These exhaustively explore thread
//! interleavings of the pool's lock-free free list, per-slot refcounts, and the
//! pool-refcount teardown — the parts exercised by concurrent `Box`/`Arc`
//! drops.
//!
//! The whole file is gated on `--cfg loom`, so a normal `cargo test` sees an
//! empty target. Run with:
//!
//! ```sh
//! RUSTFLAGS="--cfg loom" cargo test --test loom_pool --features loom --release
//! ```
#![cfg(loom)]

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator, Global};
use loom::sync::Arc as LoomArc;
use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::thread;
use plurality::Pool;

/// Value whose `Drop` records into a loom-tracked counter, so we can assert a
/// value is destroyed exactly once across all interleavings.
struct Tracked(LoomArc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

struct GrowthRaceAllocator {
    allocations: LoomArc<AtomicUsize>,
    stage: LoomArc<AtomicUsize>,
}

// SAFETY: allocations and deallocations are forwarded unchanged to `Global`;
// the loom state only coordinates when the second allocation returns.
unsafe impl Allocator for GrowthRaceAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let allocation = Global.allocate(layout)?;
        if self.allocations.fetch_add(1, Ordering::Relaxed) == 1 {
            self.stage.store(1, Ordering::Release);
            while self.stage.load(Ordering::Acquire) != 2 {
                thread::yield_now();
            }
        }
        Ok(allocation)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: `ptr` was returned by `Global::allocate` above with `layout`.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

/// Two `Arc` handles to the *same* slot, dropped from two threads. Exercises the
/// per-slot refcount `fetch_sub` race and the subsequent free-list push.
#[test]
fn two_arcs_same_slot() {
    loom::model(|| {
        let pool = Pool::<u32>::builder().chunk_size(2).build();
        let a = pool.alloc_arc(7);
        let b = a.clone();

        let t = thread::spawn(move || {
            assert_eq!(*b, 7);
            drop(b);
        });

        assert_eq!(*a, 7);
        drop(a);
        t.join().unwrap();

        drop(pool);
    });
}

/// The last handle is dropped on a worker thread *after* the `Pool` handle is
/// gone, so teardown runs cross-thread. Exercises the pool-refcount release /
/// acquire fence.
#[test]
fn teardown_on_worker_thread() {
    loom::model(|| {
        let pool = Pool::<u32>::builder().chunk_size(2).build();
        let a = pool.alloc_arc(99);
        drop(pool); // pool handle gone; `a` keeps the backing alive

        let t = thread::spawn(move || {
            assert_eq!(*a, 99);
            drop(a); // last reference -> teardown happens here
        });
        t.join().unwrap();
    });
}

/// Two *distinct* slots freed concurrently — two producers pushing onto the MPSC
/// free list at once.
#[test]
fn concurrent_frees_distinct_slots() {
    loom::model(|| {
        let pool = Pool::<u32>::builder().chunk_size(2).build();
        let a = pool.alloc_box(1);
        let b = pool.alloc_box(2);

        let t1 = thread::spawn(move || drop(a));
        let t2 = thread::spawn(move || drop(b));
        t1.join().unwrap();
        t2.join().unwrap();

        drop(pool);
    });
}

/// Pauses second-chunk allocation while another thread frees an old slot. The
/// subsequent splice must retain both the new chunk's free chain and the
/// concurrently published old slot.
#[test]
fn free_during_growth_is_preserved_by_splice() {
    loom::model(|| {
        let allocations = LoomArc::new(AtomicUsize::new(0));
        let stage = LoomArc::new(AtomicUsize::new(0));
        let allocator = GrowthRaceAllocator {
            allocations: LoomArc::clone(&allocations),
            stage: LoomArc::clone(&stage),
        };
        let pool = Pool::<u32>::builder().chunk_size(2).allocator(allocator).build();
        let first = pool.alloc_box(1);
        let second = pool.alloc_box(2);

        let worker_stage = LoomArc::clone(&stage);
        let worker = thread::spawn(move || {
            while worker_stage.load(Ordering::Acquire) != 1 {
                thread::yield_now();
            }
            drop(first);
            worker_stage.store(2, Ordering::Release);
        });

        let third = pool.alloc_box(3);
        worker.join().unwrap();
        assert_eq!(pool.chunks_allocated(), 2);

        let fourth = pool.alloc_box(4);
        let fifth = pool.alloc_box(5);
        assert_eq!(pool.chunks_allocated(), 2, "growth splice lost the concurrently freed slot");

        drop((second, third, fourth, fifth));
        assert_eq!(pool.len(), 0);
    });
}

/// Like `two_arcs_same_slot`, but asserts the value is dropped exactly once
/// regardless of which thread releases the last reference.
#[test]
fn value_dropped_exactly_once() {
    loom::model(|| {
        let drops = LoomArc::new(AtomicUsize::new(0));
        let pool = Pool::<Tracked>::builder().chunk_size(2).build();

        let a = pool.alloc_arc(Tracked(drops.clone()));
        let b = a.clone();

        let t = thread::spawn(move || drop(b));
        drop(a);
        t.join().unwrap();

        assert_eq!(drops.load(Ordering::Acquire), 1, "value must drop exactly once");
        drop(pool);
    });
}
