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

//! Tests for the `Arc` handle: shared ownership, atomic refcounting, and the
//! uninitialized placement API.

mod common;

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::DropCounter;
use plurality::{Arc, Pool};

#[test]
fn arc_share_and_clone() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();
    let a = pool.alloc_arc(7);
    let a2 = a.clone();
    let a3 = a2.clone();
    assert_eq!(*a, 7);
    assert_eq!(*a3, 7);
    assert_eq!(pool.len(), 1); // one slot, three handles
    drop(a);
    drop(a2);
    assert_eq!(pool.len(), 1);
    drop(a3);
    assert_eq!(pool.len(), 0);
}

#[test]
fn arc_drops_value_on_last_handle() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(2).build();
    let a = pool.alloc_arc(DropCounter(counter.clone()));
    let a2 = a.clone();
    drop(a);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(a2);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn uninit_arc_placement() {
    let pool = Pool::<String>::builder().chunk_size(2).build();
    let mut a = pool.alloc_uninit_arc();
    Arc::get_mut(&mut a).unwrap().write(String::from("hello"));
    // SAFETY: the value was written just above.
    let a = unsafe { a.assume_init() };
    let a2 = a.clone();
    assert_eq!(a.as_str(), "hello");
    assert_eq!(a2.as_str(), "hello");
    drop((a, a2));
    assert_eq!(pool.len(), 0);
}
