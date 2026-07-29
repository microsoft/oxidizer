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

//! Property tests for mixed allocation, clone, and drop sequences. Every value
//! must be dropped once, and releasing all handles must empty the pool.

// Bolero needs filesystem isolation unavailable under Miri.
#![cfg(not(miri))]

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use plurality::{Arc, Box, Pool};

/// Value that records its destruction into a shared counter.
struct Tracked(StdArc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[allow(dead_code, reason = "handles are held for their ownership/drop side effects")]
enum Handle {
    Boxed(Box<Tracked>),
    Shared(Arc<Tracked>),
}

/// Interprets `input` as an op stream and checks the invariants.
fn run(input: &[u8]) {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<Tracked>::builder().chunk_size(4).build();
    let mut handles: Vec<Handle> = Vec::new();
    let mut allocations = 0usize;

    let mut bytes = input.iter().copied();
    while let Some(cmd) = bytes.next() {
        match cmd % 4 {
            0 => {
                handles.push(Handle::Boxed(pool.alloc_box(Tracked(counter.clone()))));
                allocations += 1;
            }
            1 => {
                handles.push(Handle::Shared(pool.alloc_arc(Tracked(counter.clone()))));
                allocations += 1;
            }
            2 => {
                if !handles.is_empty() {
                    let idx = bytes.next().unwrap_or(0) as usize % handles.len();
                    if let Handle::Shared(a) = &handles[idx] {
                        handles.push(Handle::Shared(a.clone()));
                    }
                }
            }
            _ => {
                if !handles.is_empty() {
                    let idx = bytes.next().unwrap_or(0) as usize % handles.len();
                    drop(handles.swap_remove(idx));
                }
            }
        }
    }

    drop(handles);

    assert_eq!(
        counter.load(Ordering::Relaxed),
        allocations,
        "expected {allocations} drops, saw {}",
        counter.load(Ordering::Relaxed)
    );
    assert_eq!(pool.len(), 0, "pool should have no live allocations");
}

#[test]
fn pool_invariants() {
    bolero::check!().for_each(|input: &[u8]| run(input));
}
