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

//! Tests for the `Rc` handle: shared, non-atomic refcounting, `get_mut`, and
//! the non-atomic-to-atomic handoff when a freed slot is reused by an `Arc`.

mod common;

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::DropCounter;
use plurality::Pool;

#[test]
fn rc_share_clone_and_drop() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(4).build();
    let r = pool.alloc_rc(DropCounter(counter.clone()));
    let r2 = r.clone();
    let r3 = r2.clone();
    assert_eq!(pool.len(), 1); // one slot, three handles
    drop(r);
    drop(r2);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(r3);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);
}

#[test]
fn rc_get_mut_when_unique() {
    let pool = Pool::<u32>::builder().chunk_size(2).build();
    let mut r = pool.alloc_rc(1);
    *plurality::Rc::get_mut(&mut r).unwrap() = 42;
    assert_eq!(*r, 42);
    let r2 = r.clone();
    assert!(plurality::Rc::get_mut(&mut r).is_none()); // shared now
    drop(r2);
    assert!(plurality::Rc::get_mut(&mut r).is_some()); // unique again
}

#[test]
fn rc_outlives_pool() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(2).build();
    let r = pool.alloc_rc(DropCounter(counter.clone()));
    let r2 = r.clone();
    drop(pool); // pool handle gone; Rc keeps the backing alive
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(r);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(r2);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn rc_freed_slot_reused_as_cross_thread_arc() {
    // An Rc frees its slot (non-atomic dec, then atomic push); the slot is then
    // reused by an Arc that is dropped on another thread (atomic). Exercises the
    // non-atomic -> atomic handoff on a reused slot across threads.
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(1).max_chunks(1).build();

    let r = pool.alloc_rc(DropCounter(counter.clone()));
    let r2 = r.clone();
    drop(r);
    drop(r2); // slot freed via the non-atomic Rc path
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let a = pool.alloc_arc(DropCounter(counter.clone())); // reuses the same slot
    std::thread::spawn(move || drop(a)).join().unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert_eq!(pool.len(), 0);
}
