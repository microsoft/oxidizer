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
    reason = "test code"
)]

//! Tests for the `Alloc` handle: the lifetime-bound, unique owner that borrows
//! the pool and skips the pool reference count.

mod common;

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::DropCounter;
use plurality::Pool;

#[test]
fn alloc_deref_mutate_and_reuse() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();
    {
        let mut a = pool.alloc(10);
        assert_eq!(*a, 10);
        *a += 5;
        assert_eq!(*a, 15);
    }
    // Alloc is not counted by len() (it skips pool_refcount), but it does free
    // its slot on drop — so repeated scoped allocs reuse one chunk.
    for i in 0..100u32 {
        let a = pool.alloc(i);
        assert_eq!(*a, i);
    }
    assert_eq!(pool.chunks_allocated(), 1);
}

#[test]
fn alloc_runs_drop() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(4).build();
    {
        let _a = pool.alloc(DropCounter(counter.clone()));
        let _b = pool.alloc_with(|| DropCounter(counter.clone()));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn alloc_uninit_placement() {
    let pool = Pool::<u64>::builder().chunk_size(4).build();
    let mut a = pool.alloc_uninit();
    a.write(7);
    // SAFETY: the value was written just above.
    let a = unsafe { a.assume_init() };
    assert_eq!(*a, 7);
}

struct PanicOnce(StdArc<AtomicBool>);

impl Drop for PanicOnce {
    fn drop(&mut self) {
        // This value panics the first time it is dropped and returns normally
        // on every later drop, exercising an unwind through the pool's reclaim
        // path. `swap` returns the previous flag: it is `false` on the first
        // drop (so the assert fails and unwinds) and `true` afterwards (so the
        // assert passes).
        let dropped_before = self.0.swap(true, Ordering::SeqCst);
        assert!(dropped_before, "PanicOnce panics on its first drop");
    }
}

#[test]
fn panicking_destructor_returns_local_slot() {
    let panicked = StdArc::new(AtomicBool::new(false));
    let pool = Pool::<PanicOnce>::builder().chunk_size(1).max_chunks(1).build();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(pool.alloc(PanicOnce(panicked.clone())));
        }))
        .is_err()
    );
    drop(pool.alloc(PanicOnce(panicked)));
}
