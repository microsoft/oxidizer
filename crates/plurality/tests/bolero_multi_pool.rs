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

//! Property tests for mixed allocation, clone, and drop sequences over a multi
//! pool. Every value must be dropped once, releasing all handles must empty the
//! pool, and a slot must only ever be reused within its own layout.

// Bolero needs filesystem isolation unavailable under Miri.
#![cfg(not(miri))]

use std::collections::HashMap;
use std::ptr::from_ref;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use plurality::{Arc, Box, MultiPool};

/// Values recording their destruction into a shared counter, over a spread of
/// layouts. The pool must keep their slots apart even though one pool serves
/// them all.
macro_rules! tracked {
    ($name:ident, $payload:ty $(, align($align:literal))?) => {
        $(#[repr(align($align))])?
        struct $name(StdArc<AtomicUsize>, #[allow(dead_code, reason = "present to give the type its layout")] $payload);

        impl Drop for $name {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
}

tracked!(Narrow, u8);
tracked!(Wide, [u64; 4]);
tracked!(Aligned, u8, align(64));

/// Handles are held for their ownership and drop side effects; the layout tag
/// records which layout the slot behind each one belongs to.
#[allow(dead_code, reason = "handles are held for their ownership/drop side effects")]
enum Handle {
    NarrowBoxed(Box<Narrow>),
    NarrowShared(Arc<Narrow>),
    WideBoxed(Box<Wide>),
    WideShared(Arc<Wide>),
    AlignedBoxed(Box<Aligned>),
    AlignedShared(Arc<Aligned>),
}

/// Interprets `input` as an op stream and checks the invariants.
fn run(input: &[u8]) {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = MultiPool::builder().chunk_size(4).build();
    let mut handles: Vec<Handle> = Vec::new();
    let mut allocations = 0_usize;

    // Address of every slot ever handed out, against the layout that owns it.
    // Layout pools never share a chunk and never release one, so an address
    // that shows up under two layouts is a slot that crossed layouts.
    let mut owner: HashMap<usize, u8> = HashMap::new();
    let mut claim = |address: usize, layout: u8| {
        let previous = owner.insert(address, layout);
        assert!(
            previous.is_none_or(|seen| seen == layout),
            "slot {address:#x} was served for layout {previous:?} and then for layout {layout}"
        );
    };

    let mut bytes = input.iter().copied();
    while let Some(cmd) = bytes.next() {
        match cmd % 8 {
            0 => {
                let held = pool.alloc_box(Narrow(counter.clone(), 1));
                claim(from_ref(&*held) as usize, 0);
                handles.push(Handle::NarrowBoxed(held));
                allocations += 1;
            }
            1 => {
                let held = pool.alloc_arc(Narrow(counter.clone(), 1));
                claim(from_ref(&*held) as usize, 0);
                handles.push(Handle::NarrowShared(held));
                allocations += 1;
            }
            2 => {
                let held = pool.alloc_box(Wide(counter.clone(), [2; 4]));
                claim(from_ref(&*held) as usize, 1);
                handles.push(Handle::WideBoxed(held));
                allocations += 1;
            }
            3 => {
                let held = pool.alloc_arc(Wide(counter.clone(), [2; 4]));
                claim(from_ref(&*held) as usize, 1);
                handles.push(Handle::WideShared(held));
                allocations += 1;
            }
            4 => {
                let held = pool.alloc_box(Aligned(counter.clone(), 3));
                claim(from_ref(&*held) as usize, 2);
                handles.push(Handle::AlignedBoxed(held));
                allocations += 1;
            }
            5 => {
                let held = pool.alloc_arc(Aligned(counter.clone(), 3));
                claim(from_ref(&*held) as usize, 2);
                handles.push(Handle::AlignedShared(held));
                allocations += 1;
            }
            6 => {
                if !handles.is_empty() {
                    let idx = bytes.next().unwrap_or(0) as usize % handles.len();
                    let clone = match &handles[idx] {
                        Handle::NarrowShared(a) => Some(Handle::NarrowShared(a.clone())),
                        Handle::WideShared(a) => Some(Handle::WideShared(a.clone())),
                        Handle::AlignedShared(a) => Some(Handle::AlignedShared(a.clone())),
                        _ => None,
                    };
                    handles.extend(clone);
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
    assert!(pool.layouts() <= 3, "the op stream only ever asks for three layouts");
}

#[test]
fn multi_pool_invariants() {
    bolero::check!().for_each(|input: &[u8]| run(input));
}
