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

//! Tests for the `Box` handle: owned allocation, deref/mutation, and the
//! uninitialized placement API.

mod common;

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::DropCounter;
use plurality::{Box as PoolBox, Pool};

#[test]
fn box_alloc_deref_mutate() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();
    let mut b = pool.alloc_box(10);
    assert_eq!(*b, 10);
    *b += 5;
    assert_eq!(*b, 15);
    assert_eq!(pool.len(), 1);
    drop(b);
    assert_eq!(pool.len(), 0);
}

#[test]
fn uninit_box_placement() {
    let pool = Pool::<u64>::builder().chunk_size(4).build();
    let mut b = pool.alloc_uninit_box();
    b.write(0xDEAD_BEEF);
    // SAFETY: the value was written just above.
    let b = unsafe { b.assume_init() };
    assert_eq!(*b, 0xDEAD_BEEF);
    assert_eq!(pool.len(), 1);
    drop(b);
    assert_eq!(pool.len(), 0);
}

#[test]
fn uninit_box_dropped_without_init() {
    // Reserving an uninit slot and dropping it must free the slot and must not
    // run any destructor (the value was never written).
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(2).build();
    let b = pool.alloc_uninit_box();
    assert_eq!(pool.len(), 1);
    drop(b); // no value to drop
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(pool.len(), 0);
    // The freed slot is reusable.
    let c = pool.alloc_box(DropCounter(counter.clone()));
    drop(c);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn uninit_handle_outlives_pool() {
    let pool = Pool::<u32>::builder().chunk_size(2).build();
    let mut b = pool.alloc_uninit_box();
    b.write(99);
    drop(pool); // teardown is reached via the uninit/value box path
    // SAFETY: the value was written just above.
    let b = unsafe { b.assume_init() };
    assert_eq!(*b, 99);
    drop(b);
}

#[test]
fn into_raw_from_raw_round_trips_and_frees() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(2).build();
    let b = pool.alloc_box(DropCounter(counter.clone()));
    assert_eq!(pool.len(), 1);

    // Leaking to a raw pointer keeps the slot occupied and runs no destructor.
    let raw = PoolBox::into_raw(b);
    assert_eq!(pool.len(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Reconstructing and dropping runs the destructor and returns the slot.
    // SAFETY: `raw` came from `into_raw` on this pool and is used exactly once.
    let b: PoolBox<DropCounter> = unsafe { PoolBox::from_raw(raw) };
    drop(b);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);

    // The freed slot is reusable.
    let c = pool.alloc_box(DropCounter(counter.clone()));
    drop(c);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn into_raw_pointer_is_stable_and_readable() {
    let pool = Pool::<u64>::builder().chunk_size(4).build();
    let raw = PoolBox::into_raw(pool.alloc_box(0xABCD_1234));
    // SAFETY: the slot is kept occupied by the outstanding raw pointer.
    assert_eq!(unsafe { *raw.as_ptr() }, 0xABCD_1234);
    // SAFETY: reconstructed exactly once to free the slot.
    let b: PoolBox<u64> = unsafe { PoolBox::from_raw(raw) };
    drop(b);
    assert_eq!(pool.len(), 0);
}
