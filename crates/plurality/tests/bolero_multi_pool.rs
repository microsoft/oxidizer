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
//! pool, a slot must only ever be reused within its own layout, a slot must
//! never be served while a handle to it is still live, and every handle must
//! read back the payload its own allocation wrote.

// Bolero needs filesystem isolation unavailable under Miri.
#![cfg(not(miri))]

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
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
        struct $name(StdArc<AtomicUsize>, $payload);

        impl $name {
            fn new(counter: &StdArc<AtomicUsize>, stamp: u64) -> Self {
                Self(counter.clone(), <$payload as Payload>::fill(stamp))
            }

            fn verify(&self, stamp: u64) {
                assert_eq!(
                    self.1,
                    <$payload as Payload>::fill(stamp),
                    "the value allocated with stamp {stamp} reads back a foreign payload"
                );
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
}

tracked!(Narrow, u64);
tracked!(Wide, [u64; 4]);
tracked!(Aligned, u64, align(64));

/// Payload occupying a tracked value's layout, derived from a stamp.
///
/// The stamp fills the payload rather than a single leading word so that a
/// readback covers every byte the slot's geometry spans, catching a write that
/// landed with the wrong geometry as well as one that landed in the wrong slot.
trait Payload: PartialEq + Debug {
    fn fill(stamp: u64) -> Self;
}

impl Payload for u64 {
    fn fill(stamp: u64) -> Self {
        stamp
    }
}

impl Payload for [u64; 4] {
    fn fill(stamp: u64) -> Self {
        [stamp; 4]
    }
}

/// A handle in the flavour the op stream asked for.
enum Handle {
    NarrowBoxed(Box<Narrow>),
    NarrowShared(Arc<Narrow>),
    WideBoxed(Box<Wide>),
    WideShared(Arc<Wide>),
    AlignedBoxed(Box<Aligned>),
    AlignedShared(Arc<Aligned>),
}

/// A handle the op stream still holds, tagged with the identity of the
/// allocation behind it.
///
/// Cloning an `Arc` adds an entry that shares the slot of the entry it was
/// cloned from, so the address is unique among allocations but not among
/// entries; the stamp identifies the value the slot must still contain.
struct Entry {
    handle: Handle,
    address: usize,
    stamp: u64,
}

impl Entry {
    /// Reads the value back through this handle.
    ///
    /// Called when an entry is cloned or released rather than over all live
    /// entries each iteration, which would make the op stream quadratic.
    fn verify(&self) {
        match &self.handle {
            Handle::NarrowBoxed(v) => v.verify(self.stamp),
            Handle::NarrowShared(v) => v.verify(self.stamp),
            Handle::WideBoxed(v) => v.verify(self.stamp),
            Handle::WideShared(v) => v.verify(self.stamp),
            Handle::AlignedBoxed(v) => v.verify(self.stamp),
            Handle::AlignedShared(v) => v.verify(self.stamp),
        }
    }
}

/// Slot bookkeeping the pool is checked against.
#[derive(Default)]
struct Oracle {
    /// Layout owning each address ever served. Layout pools never share a chunk
    /// and never release one, so an address that shows up under two layouts is
    /// a slot that crossed layouts.
    owner: HashMap<usize, u8>,

    /// Entries referencing each address currently held. An `Arc` clone shares a
    /// slot without allocating, so a slot stays live until the last entry
    /// referencing it goes away, which makes this a count and not a set.
    live: HashMap<usize, usize>,

    /// Layout tags the op stream asked for.
    requested: HashSet<u8>,
}

impl Oracle {
    /// Records a fresh allocation of `address` under `layout`.
    fn claim(&mut self, address: usize, layout: u8) {
        let previous = self.owner.insert(address, layout);
        assert!(
            previous.is_none_or(|seen| seen == layout),
            "slot {address:#x} was served for layout {previous:?} and then for layout {layout}"
        );
        assert!(
            self.live.insert(address, 1).is_none(),
            "slot {address:#x} was served again while a handle to it was still live"
        );
        _ = self.requested.insert(layout);
    }

    /// Records another entry referencing an already live `address`.
    fn retain(&mut self, address: usize) {
        *self.live.get_mut(&address).unwrap() += 1;
    }

    /// Records that one entry referencing `address` went away.
    fn release(&mut self, address: usize) {
        let entries = self.live.get_mut(&address).unwrap();
        *entries -= 1;
        if *entries == 0 {
            _ = self.live.remove(&address);
        }
    }

    /// Number of distinct layouts the op stream asked for.
    fn layouts(&self) -> usize {
        self.requested.len()
    }
}

/// Layout tags telling the three payload layouts apart. The values carry no
/// meaning beyond being distinct.
const NARROW: u8 = 0;
const WIDE: u8 = 1;
const ALIGNED: u8 = 2;

/// Interprets `input` as an op stream and checks the invariants.
fn run(input: &[u8]) {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = MultiPool::builder().chunk_size(4).build();
    let mut handles: Vec<Entry> = Vec::new();
    let mut allocations = 0_usize;
    let mut oracle = Oracle::default();

    // Every allocation carries a stamp no other allocation carries, so a
    // readback distinguishes this value from any other value the pool holds.
    let mut stamps = 0_u64..;

    // The allocating arms differ only in the pool entry point, the value type,
    // and the handle variant, so they share one expansion over the op stream's
    // locals.
    macro_rules! alloc {
        ($method:ident, $value:ident, $variant:ident, $layout:ident) => {{
            let stamp = stamps.next().unwrap();
            let held = pool.$method($value::new(&counter, stamp));
            let address = from_ref(&*held) as usize;
            oracle.claim(address, $layout);
            handles.push(Entry {
                handle: Handle::$variant(held),
                address,
                stamp,
            });
            allocations += 1;
        }};
    }

    let mut bytes = input.iter().copied();
    while let Some(cmd) = bytes.next() {
        match cmd % 8 {
            0 => alloc!(alloc_box, Narrow, NarrowBoxed, NARROW),
            1 => alloc!(alloc_arc, Narrow, NarrowShared, NARROW),
            2 => alloc!(alloc_box, Wide, WideBoxed, WIDE),
            3 => alloc!(alloc_arc, Wide, WideShared, WIDE),
            4 => alloc!(alloc_box, Aligned, AlignedBoxed, ALIGNED),
            5 => alloc!(alloc_arc, Aligned, AlignedShared, ALIGNED),
            6 => {
                if !handles.is_empty() {
                    let idx = bytes.next().unwrap_or(0) as usize % handles.len();
                    let entry = &handles[idx];
                    let address = entry.address;
                    let stamp = entry.stamp;
                    let clone = match &entry.handle {
                        Handle::NarrowShared(a) => Some(Handle::NarrowShared(a.clone())),
                        Handle::WideShared(a) => Some(Handle::WideShared(a.clone())),
                        Handle::AlignedShared(a) => Some(Handle::AlignedShared(a.clone())),
                        _ => None,
                    };
                    entry.verify();
                    if let Some(handle) = clone {
                        oracle.retain(address);
                        handles.push(Entry { handle, address, stamp });
                    }
                }
            }
            _ => {
                if !handles.is_empty() {
                    let idx = bytes.next().unwrap_or(0) as usize % handles.len();
                    let entry = handles.swap_remove(idx);
                    entry.verify();
                    oracle.release(entry.address);
                }
            }
        }
    }

    while let Some(entry) = handles.pop() {
        entry.verify();
        oracle.release(entry.address);
    }

    assert_eq!(
        counter.load(Ordering::Relaxed),
        allocations,
        "expected {allocations} drops, saw {}",
        counter.load(Ordering::Relaxed)
    );
    assert_eq!(pool.len(), 0, "pool should have no live allocations");
    assert!(oracle.live.is_empty(), "every handle was released, so no slot may remain live");
    assert_eq!(
        pool.layouts(),
        oracle.layouts(),
        "the pool must hold one layout pool per layout the op stream asked for"
    );
}

#[test]
fn multi_pool_invariants() {
    bolero::check!().for_each(|input: &[u8]| run(input));
}
