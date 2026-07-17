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
