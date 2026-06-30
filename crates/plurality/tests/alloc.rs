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
use std::sync::atomic::{AtomicUsize, Ordering};

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
