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

//! Tests for the `BlindPool` and `BlindPoolBuilder`: heterogeneous mixes, types
//! sharing one layout, zero-sized and over-aligned values, coercion, handles
//! outliving the pool, per-layout and per-pool capacity limits, the sizing
//! clamps, allocator failure on both the chunk and the metadata paths, panic
//! safety in construction closures, and reentrancy at every point the cold path
//! releases control.

mod common;

use core::alloc::Layout;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::ptr::{NonNull, from_ref};
use std::array::from_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::{Rc as StdRc, Weak as StdWeak};
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use allocator_api2::alloc::{AllocError as BackingAllocError, Allocator, Global};
use common::DropCounter;
use plurality::{Arc as PoolArc, BlindPool, Box as PoolBox, Coercion, Pool, Rc as PoolRc, coerce};

// ── the layout spread ────────────────────────────────────────────────────
//
// These types cover a spread of sizes and alignments. The blind pool derives
// slot geometry from a runtime `Layout` while every handle recomputes it from
// the compiler's view of the value, so a spread is what makes the two providers
// disagree if they ever diverge.
// Ref: docs/implementation/verification.md, "Test targets".

/// Size 0, alignment 1.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Zst;

/// A one-byte value at alignment 8. Alignment rounds its size up with it, so
/// its layout is 8/8 — size is always a multiple of alignment.
#[repr(align(8))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct ByteAtEightAlign(u8);

/// Size 16, alignment 16.
#[repr(align(16))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wide16([u8; 16]);

/// Size 24, alignment 8.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Triple(u64, u64, u64);

/// Size 64, alignment 64: a value whose alignment exceeds its payload, so its
/// slot is padded and its chunk reserves alignment-sized padding ahead of the
/// first slot. Ref: docs/design/blind-pool.md, "Exact layouts, no size classes".
#[repr(align(64))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct OverAligned(u8);

/// Two unrelated types of one layout, for the sharing tests.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Meters(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Seconds(u64);

/// A layout so large that a chunk of the requested slot count could not have a
/// representable memory layout, which forces the chunk-size clamp. The exponent
/// is derived from the target's pointer width so that the arithmetic overflows
/// on every target. Ref: docs/design/blind-pool.md, "Clamping and effective
/// sizing".
type Huge = [u8; 1_usize << (usize::BITS as usize / 2 + 8)];

// ── construction, introspection & formatting ─────────────────────────────

#[test]
fn constructors_and_builder() {
    let _ = BlindPool::new();
    let _ = BlindPool::default();
    // The builder is obtained from its target type, not constructed directly.
    let _ = BlindPool::builder();

    assert_eq!(BlindPool::new().max_layouts(), None);

    // `chunk_size` replaces an earlier `chunk_bytes`, and `allocator` swaps the
    // allocator type while carrying the rest of the configuration over.
    let pool = BlindPool::builder()
        .chunk_bytes(8192)
        .chunk_size(8)
        .max_chunks(4)
        .max_layouts(16)
        .allocator(Global)
        .build();
    assert_eq!(pool.max_layouts(), Some(16));
    assert_eq!(pool.chunk_size_of::<u32>(), 8);
    assert_eq!(pool.max_chunks_of::<u32>(), 4);
    assert_eq!(*pool.alloc_box(1_u32), 1);
}

#[test]
fn debug_reports_directory_state() {
    let builder = BlindPool::builder().max_layouts(2);
    assert!(format!("{builder:?}").contains("BlindPoolBuilder"));

    let pool = builder.build();
    let _held = pool.alloc_box(1_u64);
    let rendered = format!("{pool:?}");
    assert!(rendered.contains("BlindPool"));
    assert!(rendered.contains("layouts"));
    assert!(rendered.contains("chunks_allocated"));
}

#[test]
fn aggregate_queries_sum_across_layouts() {
    let pool = BlindPool::builder().chunk_size(2).build();
    assert_eq!(pool.layouts(), 0);
    assert_eq!(pool.len(), 0);
    assert!(pool.is_empty());
    assert_eq!(pool.chunks_allocated(), 0);
    assert_eq!(pool.capacity(), 0);
    assert_eq!(pool.max_layouts(), None);

    let a = pool.alloc_box(1_u8);
    let b = pool.alloc_arc(2_u64);
    let c = pool.alloc_rc(String::from("x"));
    // An `Alloc` occupies a slot but is not counted by `len`.
    let d = pool.alloc(4_u16);

    assert_eq!(pool.layouts(), 4);
    assert_eq!(pool.len(), 3);
    assert!(!pool.is_empty());
    assert_eq!(pool.chunks_allocated(), 4);
    assert_eq!(pool.capacity(), 8);
    assert_eq!(
        pool.capacity(),
        pool.capacity_of::<u8>() + pool.capacity_of::<u64>() + pool.capacity_of::<String>() + pool.capacity_of::<u16>()
    );
    assert_eq!(
        pool.chunks_allocated(),
        u64::from(pool.chunks_allocated_of::<u8>())
            + u64::from(pool.chunks_allocated_of::<u64>())
            + u64::from(pool.chunks_allocated_of::<String>())
            + u64::from(pool.chunks_allocated_of::<u16>())
    );
    assert_eq!(pool.len(), pool.len_of::<u8>() + pool.len_of::<u64>() + pool.len_of::<String>());

    drop((a, b, c, d));
    assert!(pool.is_empty());
    // A layout pool is never retired and never returns chunk memory, so the
    // reported figures are a high-water mark.
    // Ref: docs/design/blind-pool.md, "Memory is monotonic per layout".
    assert_eq!(pool.layouts(), 4);
    assert_eq!(pool.chunks_allocated(), 4);
    assert_eq!(pool.capacity(), 8);
}

#[test]
fn per_layout_queries_report_an_unseen_layout_without_creating_one() {
    let pool = BlindPool::builder().chunk_bytes(4096).max_chunks(2).build();

    assert_eq!(pool.len_of::<u64>(), 0);
    assert_eq!(pool.chunks_allocated_of::<u64>(), 0);
    assert_eq!(pool.capacity_of::<u64>(), 0);

    // The effective sizing is reported for a layout the pool has never served.
    // Ref: docs/design/blind-pool.md, "Allocation surface".
    let chunk_size = pool.chunk_size_of::<u64>();
    assert!(chunk_size.is_power_of_two());
    assert_eq!(pool.max_chunks_of::<u64>(), 2);
    // A larger layout takes fewer slots out of the same byte target.
    assert!(pool.chunk_size_of::<[u64; 64]>() < chunk_size);
    assert_eq!(pool.layouts(), 0, "a query must not create a layout pool");

    let _held = pool.alloc_box(1_u64);
    assert_eq!(pool.layouts(), 1);
    assert_eq!(
        pool.chunk_size_of::<u64>(),
        chunk_size,
        "the built pool must use the reported sizing"
    );
    assert_eq!(pool.max_chunks_of::<u64>(), 2);
    assert_eq!(pool.chunks_allocated_of::<u64>(), 1);
    assert_eq!(pool.capacity_of::<u64>(), u64::from(chunk_size));
    assert_eq!(pool.len_of::<u64>(), 1);
}

// ── heterogeneous pooling ────────────────────────────────────────────────

#[test]
fn unrelated_types_live_in_one_pool_at_once() {
    let pool = BlindPool::new();

    let count = pool.alloc_box(42_u64);
    let name = pool.alloc_box(String::from("hello"));
    let ratio = pool.alloc_arc(7_i32);
    let padded = pool.alloc_rc(OverAligned(9));
    let marker = pool.alloc(Zst);

    assert_eq!(*count, 42);
    assert_eq!(&**name, "hello");
    assert_eq!(*ratio, 7);
    assert_eq!(padded.0, 9);
    assert_eq!(*marker, Zst);
    assert_eq!(pool.layouts(), 5);
    assert_eq!(pool.len(), 4);
}

#[test]
fn distinct_types_of_one_layout_share_a_layout_pool() {
    assert_eq!(size_of::<Meters>(), size_of::<Seconds>());
    assert_eq!(align_of::<Meters>(), align_of::<Seconds>());

    let pool = BlindPool::new();
    let mut distances = Vec::new();
    let mut durations = Vec::new();
    // Interleaved so the two types draw alternating slots from one chunk.
    for i in 0..8_u64 {
        distances.push(pool.alloc_box(Meters(i)));
        durations.push(pool.alloc_box(Seconds(1000 + i)));
    }

    // Types that share a layout share a layout pool: memory sharing, not type
    // confusion. Ref: docs/design/blind-pool.md, "The router and the layout
    // pools".
    assert_eq!(pool.layouts(), 1);
    for (i, (distance, duration)) in distances.iter().zip(&durations).enumerate() {
        assert_eq!(distance.0, i as u64);
        assert_eq!(duration.0, 1000 + i as u64);
    }

    // Capacity reported for one type may be occupied by values of the other.
    assert_eq!(pool.len_of::<Meters>(), 16);
    assert_eq!(pool.len_of::<Seconds>(), pool.len_of::<Meters>());
    assert_eq!(pool.capacity_of::<Meters>(), pool.capacity_of::<Seconds>());

    drop(distances);
    assert_eq!(pool.len_of::<Seconds>(), 8);
    drop(durations);
    assert!(pool.is_empty());
}

/// Drops of the zero-sized value below, which cannot carry a counter itself.
static ZERO_SIZED_DROPS: AtomicUsize = AtomicUsize::new(0);

struct CountedZst;

impl Drop for CountedZst {
    fn drop(&mut self) {
        ZERO_SIZED_DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn zero_sized_values_are_pooled_and_reclaimed() {
    // One slot per layout, so a reclaimed slot is the only way a second
    // allocation can succeed.
    let pool = BlindPool::builder().chunk_size(1).max_chunks(1).build();

    let held = pool.alloc_box(CountedZst);
    assert!(pool.try_alloc_box(CountedZst).is_err());
    assert_eq!(ZERO_SIZED_DROPS.load(Ordering::SeqCst), 1, "the rejected value is dropped");

    drop(held);
    assert_eq!(ZERO_SIZED_DROPS.load(Ordering::SeqCst), 2);
    let reused = pool.alloc_box(CountedZst);
    assert_eq!(pool.chunks_allocated_of::<CountedZst>(), 1);
    drop(reused);
    assert_eq!(ZERO_SIZED_DROPS.load(Ordering::SeqCst), 3);
    assert!(pool.is_empty());
}

#[test]
fn over_aligned_values_are_pooled_and_reclaimed() {
    let pool = BlindPool::builder().chunk_size(1).max_chunks(1).build();

    let held = pool.alloc_box(OverAligned(7));
    assert!(PoolBox::as_ptr(&held).is_aligned());
    assert_eq!(held.0, 7);
    assert!(pool.try_alloc_box(OverAligned(8)).is_err());

    drop(held);
    let reused = pool.alloc_box(OverAligned(8));
    assert!(PoolBox::as_ptr(&reused).is_aligned());
    assert_eq!(reused.0, 8);
    assert_eq!(pool.chunks_allocated_of::<OverAligned>(), 1);
}

// ── the layout spread through `Alloc` ────────────────────────────────────

/// Runs one layout through every `Alloc` entry point.
///
/// `Alloc` is the only handle that reads its slot through the compiler's layout
/// of the slot type, so it is the only runtime check that the compiler's
/// placement and the pool's runtime-derived geometry agree.
/// Ref: docs/implementation/verification.md, "Test targets".
fn alloc_round_trip<T: Clone + PartialEq + core::fmt::Debug>(pool: &BlindPool, value: &T) {
    let by_value = pool.alloc(value.clone());
    assert_eq!(*by_value, *value);
    assert!(from_ref(&*by_value).is_aligned());

    let fallible = pool.try_alloc(value.clone()).unwrap();
    assert_eq!(*fallible, *value);

    let mut uninit = pool.alloc_uninit::<T>();
    uninit.write(value.clone());
    // SAFETY: the value was written just above.
    let placed = unsafe { uninit.assume_init() };
    assert_eq!(*placed, *value);
    assert!(from_ref(&*placed).is_aligned());

    // Three occupied slots of this layout, none of them counted: `Alloc` is
    // lifetime-bound and takes no reference on the pool.
    assert_eq!(pool.len_of::<T>(), 0);
    assert!(pool.capacity_of::<T>() >= 3);
}

#[test]
fn alloc_reads_back_a_spread_of_layouts() {
    // The spread must actually vary: a refactor that collapsed two of these
    // into one layout would silently shrink the coverage.
    assert_eq!((size_of::<u8>(), align_of::<u8>()), (1, 1));
    assert_eq!((size_of::<ByteAtEightAlign>(), align_of::<ByteAtEightAlign>()), (8, 8));
    assert_eq!((size_of::<Wide16>(), align_of::<Wide16>()), (16, 16));
    assert_eq!((size_of::<Triple>(), align_of::<Triple>()), (24, 8));
    assert_eq!((size_of::<[u64; 4]>(), align_of::<[u64; 4]>()), (32, 8));
    assert_eq!((size_of::<OverAligned>(), align_of::<OverAligned>()), (64, 64));
    assert_eq!((size_of::<Zst>(), align_of::<Zst>()), (0, 1));

    let pool = BlindPool::new();
    alloc_round_trip(&pool, &1_u8);
    alloc_round_trip(&pool, &ByteAtEightAlign(2));
    alloc_round_trip(&pool, &3_u64);
    alloc_round_trip(&pool, &Wide16([4; 16]));
    alloc_round_trip(&pool, &Triple(5, 6, 7));
    alloc_round_trip(&pool, &[8_u64; 4]);
    alloc_round_trip(&pool, &OverAligned(9));
    alloc_round_trip(&pool, &Zst);

    // `ByteAtEightAlign` and `u64` share the 8/8 layout, so eight types occupy
    // seven layout pools.
    assert_eq!(pool.layouts(), 7);
    // Every slot the spread occupied was released again.
    assert_eq!(pool.len(), 0);
}

// ── the rest of the allocation surface ───────────────────────────────────

/// Every closure, uninitialized and pinned entry point serves the layout it is
/// given, so that no method reaches its layout pool by a path of its own.
#[test]
fn closure_uninit_and_pinned_entry_points_serve_their_layout() {
    let pool = BlindPool::new();

    assert_eq!(*pool.alloc_with(|| 1_u32), 1);

    let mut ub = pool.alloc_uninit_box::<u32>();
    ub.write(2);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ub.assume_init() }, 2);

    let mut ua = pool.alloc_uninit_arc::<u32>();
    PoolArc::get_mut(&mut ua).unwrap().write(3);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ua.assume_init() }, 3);

    let mut ur = pool.alloc_uninit_rc::<u32>();
    PoolRc::get_mut(&mut ur).unwrap().write(4);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ur.assume_init() }, 4);

    assert_eq!(*pool.alloc_arc_pin(5_u32), 5);
    assert_eq!(*pool.alloc_arc_pin_with(|| 6_u32), 6);
    assert_eq!(*pool.try_alloc_arc_pin(7_u32).unwrap(), 7);
    assert_eq!(*pool.try_alloc_arc_pin_with(|| 8_u32).unwrap(), 8);

    assert_eq!(*pool.alloc_rc_pin(9_u32), 9);
    assert_eq!(*pool.alloc_rc_pin_with(|| 10_u32), 10);
    assert_eq!(*pool.try_alloc_rc_pin(11_u32).unwrap(), 11);
    assert_eq!(*pool.try_alloc_rc_pin_with(|| 12_u32).unwrap(), 12);

    // One layout served every one of them.
    assert_eq!(pool.layouts(), 1);
}

// ── coercion ─────────────────────────────────────────────────────────────

trait Shape {
    fn area(&self) -> u32;
}

struct Square(u32);

impl Shape for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}

struct Rect(u32, u32);

impl Shape for Rect {
    fn area(&self) -> u32 {
        self.0 * self.1
    }
}

#[test]
fn handles_from_one_blind_pool_erase_to_a_common_trait_object() {
    let pool = BlindPool::new();

    // Concrete types of different layouts unify behind one erased handle type,
    // which is the working set a blind pool exists to back.
    let shapes: Vec<PoolBox<dyn Shape>> = vec![
        PoolBox::unsize::<dyn Shape>(pool.alloc_box(Square(3)), coerce!(dyn Shape)),
        PoolBox::unsize::<dyn Shape>(pool.alloc_box(Rect(2, 5)), coerce!(dyn Shape)),
    ];
    assert_eq!(shapes.iter().map(|shape| shape.area()).sum::<u32>(), 19);
    assert_eq!(pool.layouts(), 2);
    assert_eq!(pool.len(), 2);

    drop(shapes);
    assert!(pool.is_empty());

    let shared: PoolArc<dyn Shape> = PoolArc::unsize::<dyn Shape>(pool.alloc_arc(Square(4)), coerce!(dyn Shape));
    let clone = shared.clone();
    assert_eq!(clone.area(), 16);
    drop(shared);
    assert_eq!(pool.len(), 1);
    drop(clone);
    assert!(pool.is_empty());

    let local: PoolRc<dyn Shape> = PoolRc::unsize::<dyn Shape>(pool.alloc_rc(Rect(3, 4)), coerce!(dyn Shape));
    assert_eq!(local.area(), 12);
    drop(local);
    assert!(pool.is_empty());
}

#[test]
fn erased_handles_run_the_concrete_destructor_and_reclaim_the_slot() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = BlindPool::new();

    // `dyn Send` has no methods, so the destructor can only be reached through
    // the value's vtable.
    let erased: PoolBox<dyn Send> = PoolBox::unsize::<dyn Send>(pool.alloc_box(DropCounter(counter.clone())), coerce!(dyn Send));
    assert_eq!(pool.len_of::<DropCounter>(), 1);
    drop(erased);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len_of::<DropCounter>(), 0);

    let shared: PoolArc<dyn Send + Sync> =
        PoolArc::unsize::<dyn Send + Sync>(pool.alloc_arc(DropCounter(counter.clone())), coerce!(dyn Send + Sync));
    let clone = shared.clone();
    drop(shared);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    drop(clone);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert!(pool.is_empty());
}

#[test]
fn arrays_from_a_blind_pool_coerce_to_slices() {
    let pool = BlindPool::new();

    let mut slice: PoolBox<[u8]> = PoolBox::unsize::<[u8]>(pool.alloc_box([1_u8, 2, 3]), Coercion::to_slice());
    assert_eq!(&*slice, &[1, 2, 3]);
    slice[0] = 9;
    assert_eq!(&*slice, &[9, 2, 3]);
    drop(slice);
    assert_eq!(pool.len_of::<[u8; 3]>(), 0);

    let counter = StdArc::new(AtomicUsize::new(0));
    let values: [DropCounter; 3] = from_fn(|_| DropCounter(counter.clone()));
    let erased: PoolBox<[DropCounter]> = PoolBox::unsize(pool.alloc_box(values), Coercion::to_slice());
    assert_eq!(erased.len(), 3);
    drop(erased);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert!(pool.is_empty());
}

// ── teardown ─────────────────────────────────────────────────────────────

#[test]
fn drops_run_exactly_once_across_layouts() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = BlindPool::builder().chunk_size(4).build();
    {
        let mut held = Vec::new();
        for _ in 0..10 {
            held.push(pool.alloc_box(DropCounter(counter.clone())));
            held.push(pool.alloc_box(DropCounter(counter.clone())));
        }
        let mut wide = Vec::new();
        for _ in 0..10 {
            wide.push(pool.alloc_arc((DropCounter(counter.clone()), 0_u64)));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(pool.len(), 30);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 30);
    assert!(pool.is_empty());
}

#[test]
fn handles_outlive_the_pool() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let (text, tracked, shared) = {
        let pool = BlindPool::new();
        let text = pool.alloc_box(String::from("outlives"));
        let tracked = pool.alloc_arc(DropCounter(counter.clone()));
        let shared = pool.alloc_rc(String::from("shared"));
        (text, tracked, shared)
    };
    // Two layout pools survive the router that created them, each tearing down
    // when its own last handle departs.
    // Ref: docs/design/blind-pool.md, "Lifetimes and teardown".
    assert_eq!(&*text, "outlives");
    assert_eq!(&*shared, "shared");
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    drop(tracked);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    drop(text);
    assert_eq!(&*shared, "shared");
    drop(shared);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// ── exhaustion ───────────────────────────────────────────────────────────

/// A blind pool giving every layout exactly one slot, plus a handle keeping
/// `u64`'s layout pool full.
fn full_pool() -> (BlindPool, PoolBox<u64>) {
    let pool = BlindPool::builder().chunk_size(1).max_chunks(1).build();
    let held = pool.alloc_box(0_u64);
    (pool, held)
}

#[test]
fn capacity_exhaustion_is_confined_to_one_layout() {
    let (pool, held) = full_pool();
    assert_eq!(pool.max_chunks_of::<u64>(), 1);
    assert_eq!(pool.capacity_of::<u64>(), 1);

    let Err(err) = pool.try_alloc_box(1_u64) else {
        panic!("the layout pool serving u64 is full");
    };
    assert!(err.is_capacity_exhausted());
    assert!(!err.is_allocator_failure());
    assert_eq!(format!("{err}"), "the pool reached its maximum capacity");

    // The cap is per layout, so an unseen layout does not compete with a full
    // one. Ref: docs/design/blind-pool.md, "Bounding growth".
    assert_eq!(*pool.alloc_box(2_u32), 2);
    assert_eq!(pool.alloc_box(Wide16([3; 16])).0, [3; 16]);
    assert_eq!(pool.layouts(), 3);

    // Freeing the one slot lets its own layout allocate again.
    drop(held);
    assert_eq!(*pool.alloc_box(4_u64), 4);
}

#[test]
fn the_layout_cap_rejects_an_unseen_layout_and_drops_its_value() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = BlindPool::builder().max_layouts(1).build();
    assert_eq!(pool.max_layouts(), Some(1));

    let seed = pool.alloc_box([1_u8, 2, 3]);
    assert_eq!(pool.layouts(), 1);

    // `DropCounter`'s layout is not the seeded one, so the request is refused
    // before a layout pool can be created for it.
    // Ref: docs/design/blind-pool.md, "Bounding growth".
    let Err(err) = pool.try_alloc_box(DropCounter(counter.clone())) else {
        panic!("the layout cap must reject an unseen layout");
    };
    assert!(err.is_capacity_exhausted());
    assert_eq!(counter.load(Ordering::SeqCst), 1, "the rejected value must be dropped exactly once");
    assert_eq!(pool.layouts(), 1);

    // An already-seen layout keeps working.
    assert_eq!(*pool.alloc_box([4_u8, 5, 6]), [4, 5, 6]);
    // A `_with` closure is not called at all.
    let mut called = false;
    assert!(
        pool.try_alloc_box_with(|| {
            called = true;
            DropCounter(counter.clone())
        })
        .is_err()
    );
    assert!(!called);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    drop(seed);
}

#[test]
fn try_alloc_reports_a_full_layout_on_every_entry_point() {
    let (pool, _held) = full_pool();

    assert!(pool.try_alloc_box(11_u64).is_err());
    assert!(pool.try_alloc_arc(12_u64).is_err());
    assert!(pool.try_alloc_arc_pin(13_u64).is_err());
    assert!(pool.try_alloc(14_u64).is_err());
    assert!(pool.try_alloc_rc(15_u64).is_err());
    assert!(pool.try_alloc_rc_pin(16_u64).is_err());

    assert!(pool.try_alloc_box_with(|| 21_u64).is_err());
    assert!(pool.try_alloc_arc_with(|| 22_u64).is_err());
    assert!(pool.try_alloc_arc_pin_with(|| 23_u64).is_err());
    assert!(pool.try_alloc_with(|| 24_u64).is_err());
    assert!(pool.try_alloc_rc_with(|| 25_u64).is_err());
    assert!(pool.try_alloc_rc_pin_with(|| 26_u64).is_err());

    assert!(pool.try_alloc_uninit_box::<u64>().is_err());
    assert!(pool.try_alloc_uninit_arc::<u64>().is_err());
    assert!(pool.try_alloc_uninit::<u64>().is_err());
    assert!(pool.try_alloc_uninit_rc::<u64>().is_err());

    // The cap binds the layout, not the pool.
    assert!(pool.try_alloc_box(31_u32).is_ok());
}

macro_rules! full_panics {
    ($name:ident, $method:ident, $arg:expr) => {
        #[test]
        #[should_panic(expected = "the pool reached its maximum capacity")]
        fn $name() {
            let (pool, _held) = full_pool();
            let _ = pool.$method($arg);
        }
    };
}

macro_rules! full_panics_uninit {
    ($name:ident, $method:ident) => {
        #[test]
        #[should_panic(expected = "the pool reached its maximum capacity")]
        fn $name() {
            let (pool, _held) = full_pool();
            let _ = pool.$method::<u64>();
        }
    };
}

full_panics!(panic_alloc_box, alloc_box, 1_u64);
full_panics!(panic_alloc_arc, alloc_arc, 1_u64);
full_panics!(panic_alloc_arc_pin, alloc_arc_pin, 1_u64);
full_panics!(panic_alloc, alloc, 1_u64);
full_panics!(panic_alloc_rc, alloc_rc, 1_u64);
full_panics!(panic_alloc_rc_pin, alloc_rc_pin, 1_u64);
full_panics!(panic_alloc_box_with, alloc_box_with, || 1_u64);
full_panics!(panic_alloc_arc_with, alloc_arc_with, || 1_u64);
full_panics!(panic_alloc_arc_pin_with, alloc_arc_pin_with, || 1_u64);
full_panics!(panic_alloc_with, alloc_with, || 1_u64);
full_panics!(panic_alloc_rc_with, alloc_rc_with, || 1_u64);
full_panics!(panic_alloc_rc_pin_with, alloc_rc_pin_with, || 1_u64);
full_panics_uninit!(panic_alloc_uninit_box, alloc_uninit_box);
full_panics_uninit!(panic_alloc_uninit_arc, alloc_uninit_arc);
full_panics_uninit!(panic_alloc_uninit, alloc_uninit);
full_panics_uninit!(panic_alloc_uninit_rc, alloc_uninit_rc);

// ── the sizing clamps ────────────────────────────────────────────────────

#[test]
fn a_huge_layout_clamps_the_requested_chunk_size_down() {
    // The largest slot count the builder accepts, asked of a layout whose chunk
    // could not be laid out at that count.
    let pool = BlindPool::builder().chunk_size(1 << 31).max_chunks(u32::MAX).build();

    let clamped = pool.chunk_size_of::<Huge>();
    assert!(clamped < 1 << 31, "the requested slot count must be clamped down");
    assert!(clamped >= 1, "a chunk always holds at least one slot");
    assert!(clamped.is_power_of_two());
    // The effective cap is the smaller of the configured cap and the ceiling
    // the clamped chunk size permits. Ref: docs/design/blind-pool.md,
    // "Clamping and effective sizing".
    assert!(
        pool.max_chunks_of::<Huge>() < u32::MAX,
        "the cap must be clamped alongside the chunk size"
    );
    assert!(pool.max_chunks_of::<Huge>() >= 1);
    assert_eq!(pool.layouts(), 0, "a query must not create a layout pool");
}

#[test]
fn a_byte_target_below_one_slot_still_buys_a_slot() {
    let pool = BlindPool::builder().chunk_bytes(1).max_chunks(3).build();

    // A value larger than the byte target on its own gets a chunk sized by the
    // value. Ref: docs/design/blind-pool.md, "Bounding growth".
    assert_eq!(pool.chunk_size_of::<u64>(), 1);
    assert_eq!(pool.max_chunks_of::<u64>(), 3);

    let held: Vec<_> = (0..3_u64).map(|i| pool.alloc_box(i)).collect();
    assert_eq!(pool.chunks_allocated_of::<u64>(), 3);
    assert_eq!(pool.capacity_of::<u64>(), 3);
    let Err(err) = pool.try_alloc_box(4_u64) else {
        panic!("three single-slot chunks are the whole allowance");
    };
    assert!(err.is_capacity_exhausted());
    drop(held);
}

#[test]
fn a_chunk_cap_beyond_the_slot_ceiling_is_clamped_down() {
    // At the largest slot count the pool will serve, the slot-index ceiling
    // permits far fewer chunks than the configured cap asks for.
    let pool = BlindPool::builder().chunk_size(1 << 31).max_chunks(100).build();

    assert!(pool.chunk_size_of::<u8>().is_power_of_two());
    assert!(
        pool.max_chunks_of::<u8>() < 100,
        "the cap must be clamped to what the slot-index ceiling permits"
    );
    assert!(pool.max_chunks_of::<u8>() >= 1);
    assert_eq!(pool.layouts(), 0, "a query must not create a layout pool");
}

#[test]
fn a_chunk_cap_of_zero_serves_nothing() {
    // A cap of zero means a pool that can never allocate, exactly as it does
    // for the typed pool. Ref: docs/design/blind-pool.md, "Bounding growth".
    let pool = BlindPool::builder().max_chunks(0).build();
    assert_eq!(pool.max_chunks_of::<u64>(), 0);
    assert_eq!(pool.capacity_of::<u64>(), 0);

    let Err(err) = pool.try_alloc_box(1_u64) else {
        panic!("a zero chunk cap leaves no room for a chunk");
    };
    assert!(err.is_capacity_exhausted());
    assert_eq!(pool.chunks_allocated_of::<u64>(), 0);

    let typed = Pool::<u64>::builder().max_chunks(0).build();
    assert_eq!(typed.max_capacity(), Some(0));
    assert!(typed.try_alloc_box(1_u64).is_err_and(plurality::AllocError::is_capacity_exhausted));
}

// ── allocator failure ────────────────────────────────────────────────────

/// An allocator that tracks live bytes, to prove memory is freed.
#[derive(Clone)]
struct CountingAllocator(StdArc<AtomicUsize>);

// SAFETY: forwards to `Global` and only adjusts a counter by the same `layout`.
unsafe impl Allocator for CountingAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, BackingAllocError> {
        let ptr = Global.allocate(layout)?;
        self.0.fetch_add(layout.size(), Ordering::SeqCst);
        Ok(ptr)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: same contract as `Global::deallocate`.
        unsafe { Global.deallocate(ptr, layout) };
        self.0.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[test]
fn dropping_the_pool_frees_every_layouts_chunks() {
    let live = StdArc::new(AtomicUsize::new(0));
    {
        let pool = BlindPool::builder()
            .chunk_size(2)
            .allocator(CountingAllocator(live.clone()))
            .build();
        let a = pool.alloc_box(1_u8);
        let b = pool.alloc_box(2_u64);
        let c = pool.alloc_box([3_u64; 4]);
        assert_eq!(pool.layouts(), 3);
        assert!(live.load(Ordering::SeqCst) > 0, "each layout must have taken a chunk");

        // Freeing slots returns them to their layout's free list; the chunks
        // stay until the pool itself goes away.
        drop((a, b, c));
        assert!(live.load(Ordering::SeqCst) > 0);
    }
    assert_eq!(live.load(Ordering::SeqCst), 0, "every layout pool must free its chunks");
}

/// A custom allocator that always fails, exercising the chunk path.
#[derive(Clone)]
struct FailingAllocator;

// SAFETY: `allocate` always returns `Err`, so no memory is ever handed out and
// `deallocate` is never called with a pointer from this allocator.
unsafe impl Allocator for FailingAllocator {
    fn allocate(&self, _layout: Layout) -> Result<NonNull<[u8]>, BackingAllocError> {
        Err(BackingAllocError)
    }

    unsafe fn deallocate(&self, _ptr: NonNull<u8>, _layout: Layout) {}
}

#[test]
fn allocator_failure_on_the_chunk_path() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = BlindPool::builder().allocator(FailingAllocator).build();

    let Err(err) = pool.try_alloc_box(1_u64) else {
        panic!("the backing allocator refuses every chunk");
    };
    assert!(err.is_allocator_failure());
    assert!(!err.is_capacity_exhausted());
    assert_eq!(format!("{err}"), "the pool could not obtain required memory");
    // The layout pool was created; only its first chunk failed.
    assert_eq!(pool.layouts(), 1);

    // A refused request drops the value it was given and never calls a `_with`
    // closure, on this path as on the capacity one.
    // Ref: docs/design/blind-pool.md, "Failure".
    assert!(pool.try_alloc_box(DropCounter(counter.clone())).is_err());
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    let mut called = false;
    assert!(
        pool.try_alloc_box_with(|| {
            called = true;
            DropCounter(counter.clone())
        })
        .is_err()
    );
    assert!(!called);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// The metadata path needs a global allocator that can be made to fail, and a
// global allocator forwarding to the system one is not meaningful under Miri's
// own allocator model. Ref: docs/implementation/verification.md,
// "Undefined-behaviour checking".
#[cfg(not(miri))]
use core::ptr::null_mut;
#[cfg(not(miri))]
use std::alloc::{GlobalAlloc, System};

// The refusal rule is thread-local so that a deny window covers only the test
// that opened it, while the rest of the target's tests run in parallel
// unaffected.
#[cfg(not(miri))]
thread_local! {
    static DENY_MODE: Cell<DenyMode> = const { Cell::new(DenyMode::Disabled) };
    static DENY_REQUESTS: Cell<usize> = const { Cell::new(0) };
    static DENIED_ALLOCATION: Cell<Option<DeniedAllocation>> = const { Cell::new(None) };
}

/// Selects the global-allocation request denied by `DenyableGlobal`.
#[cfg(not(miri))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DenyMode {
    Disabled,
    After(usize),
    Matching { layout: Layout, phase: AllocationPhase },
}

/// Identifies the allocation site a failure-injection rule is meant to reach.
#[cfg(not(miri))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AllocationPhase {
    AllocationSequence,
    DirectoryLayoutsReservation,
}

/// Records the first global allocation refused during one denial window.
#[cfg(not(miri))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeniedAllocation {
    layout: Layout,
    phase: AllocationPhase,
    preceding_requests: usize,
}

/// `true` if this allocation request is refused.
#[cfg(not(miri))]
fn refuse_allocation(layout: Layout) -> bool {
    match DENY_MODE.get() {
        DenyMode::Disabled => false,
        DenyMode::After(0) => {
            record_denial(layout, AllocationPhase::AllocationSequence);
            true
        }
        DenyMode::After(allowance) => {
            DENY_MODE.set(DenyMode::After(allowance - 1));
            DENY_REQUESTS.set(DENY_REQUESTS.get() + 1);
            false
        }
        DenyMode::Matching { layout: target, phase } if layout == target => {
            record_denial(layout, phase);
            true
        }
        DenyMode::Matching { .. } => {
            DENY_REQUESTS.set(DENY_REQUESTS.get() + 1);
            false
        }
    }
}

/// Stores only the first denial, because later failures are cascading effects.
#[cfg(not(miri))]
fn record_denial(layout: Layout, phase: AllocationPhase) {
    if DENIED_ALLOCATION.get().is_none() {
        DENIED_ALLOCATION.set(Some(DeniedAllocation {
            layout,
            phase,
            preceding_requests: DENY_REQUESTS.get(),
        }));
    }
}

/// A global allocator that can be made to fail on one thread.
///
/// Pool metadata comes from the global allocator rather than from the pool's
/// own, so a custom `A` cannot reach that path.
/// Ref: docs/implementation/blind-pool.md, "Failure".
#[cfg(not(miri))]
struct DenyableGlobal;

#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: DenyableGlobal = DenyableGlobal;

// SAFETY: every request is forwarded to `System` unchanged, except while this
// thread is refusing allocations, when a null pointer reports failure as the
// `GlobalAlloc` contract permits.
#[cfg(not(miri))]
unsafe impl GlobalAlloc for DenyableGlobal {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse_allocation(layout) {
            return null_mut();
        }
        // SAFETY: forwarded under the caller's `GlobalAlloc` contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refuse_allocation(layout) {
            return null_mut();
        }
        // SAFETY: forwarded under the caller's `GlobalAlloc` contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if refuse_allocation(layout) {
            return null_mut();
        }
        // SAFETY: forwarded under the caller's `GlobalAlloc` contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded under the caller's `GlobalAlloc` contract.
        unsafe { System.dealloc(ptr, layout) };
    }
}

/// Applies this thread's global-allocation refusal rule while held.
#[cfg(not(miri))]
struct DenyGlobal;

#[cfg(not(miri))]
impl DenyGlobal {
    fn engaged() -> Self {
        Self::after(0)
    }

    /// Lets `allowance` further allocations through before refusing.
    fn after(allowance: usize) -> Self {
        Self::with_mode(DenyMode::After(allowance))
    }

    /// Refuses the first allocation request with `layout`.
    fn matching(layout: Layout, phase: AllocationPhase) -> Self {
        Self::with_mode(DenyMode::Matching { layout, phase })
    }

    fn denied_allocation() -> Option<DeniedAllocation> {
        DENIED_ALLOCATION.get()
    }

    fn with_mode(mode: DenyMode) -> Self {
        DENIED_ALLOCATION.set(None);
        DENY_REQUESTS.set(0);
        DENY_MODE.set(mode);
        Self
    }
}

#[cfg(not(miri))]
impl Drop for DenyGlobal {
    fn drop(&mut self) {
        DENY_MODE.set(DenyMode::Disabled);
    }
}

/// Layout requested by the first reservation of the layout-key directory.
#[cfg(not(miri))]
fn first_directory_layouts_reservation_layout() -> Layout {
    first_vec_reservation_layout::<Layout>()
}

/// Asks `Vec` for its first buffer shape instead of copying its capacity rule.
#[cfg(not(miri))]
fn first_vec_reservation_layout<T>() -> Layout {
    let mut vec = Vec::<T>::new();
    vec.try_reserve(1).unwrap();
    Layout::array::<T>(vec.capacity()).unwrap()
}

#[cfg(not(miri))]
#[test]
fn allocator_failure_on_the_metadata_path() {
    let pool = BlindPool::new();
    let held = pool.alloc_box(1_u64);
    assert_eq!(pool.layouts(), 1);

    // An unseen layout must allocate its layout pool's metadata before it can
    // serve anything, and that allocation is fallible end to end.
    // Ref: docs/design/blind-pool.md, "Failure".
    let outcome = {
        let _deny = DenyGlobal::engaged();
        pool.try_alloc_box(2_u32)
    };
    let Err(err) = outcome else {
        panic!("the metadata allocation was expected to fail");
    };
    assert!(err.is_allocator_failure());
    assert!(!err.is_capacity_exhausted());
    // A failed cold path leaves the directory exactly as it was.
    assert_eq!(pool.layouts(), 1);

    // The pool serves the same layout once the allocator recovers.
    assert_eq!(*pool.alloc_box(2_u32), 2);
    assert_eq!(pool.layouts(), 2);
    drop(held);
}

#[cfg(not(miri))]
#[test]
fn allocator_failure_while_growing_the_directory() {
    let pool = BlindPool::new();
    let denied_layout = first_directory_layouts_reservation_layout();

    // The directory stores layout keys in a `Vec<Layout>`, and an empty `Vec`
    // chooses its first buffer shape from the element layout. Targeting that
    // allocation reaches the fallible reservation step without depending on
    // how many global allocations layout-pool construction uses.
    // Ref: docs/design/blind-pool.md, "Failure".
    let outcome = {
        let _deny = DenyGlobal::matching(denied_layout, AllocationPhase::DirectoryLayoutsReservation);
        pool.try_alloc_box(1_u64)
    };
    let Err(err) = outcome else {
        panic!("the directory reservation was expected to fail");
    };
    let denied = DenyGlobal::denied_allocation().unwrap();
    assert_eq!(denied.phase, AllocationPhase::DirectoryLayoutsReservation);
    assert_eq!(denied.layout, denied_layout);
    assert!(
        denied.preceding_requests > 0,
        "the denied request must follow layout-pool construction"
    );
    assert!(err.is_allocator_failure());
    assert!(!err.is_capacity_exhausted());
    // Nothing was published, so the layout is still unseen.
    assert_eq!(pool.layouts(), 0);

    // The pool serves the layout once the allocator recovers.
    assert_eq!(*pool.alloc_box(1_u64), 1);
    assert_eq!(pool.layouts(), 1);
}

// ── panic safety ─────────────────────────────────────────────────────────

#[test]
fn a_panicking_construction_closure_returns_the_slot() {
    fn check(alloc_panics: impl Fn(&BlindPool)) {
        // One slot for the layout under test, so a leaked slot makes the retry
        // below fail.
        let pool = BlindPool::builder().chunk_size(1).max_chunks(1).build();

        let panicked = catch_unwind(AssertUnwindSafe(|| alloc_panics(&pool)));
        assert!(panicked.is_err(), "the closure was expected to panic");
        assert_eq!(pool.len(), 0, "the panicked allocation must not stay live");
        assert_eq!(pool.capacity_of::<u32>(), 1, "capacity must be intact");

        let recovered = pool.try_alloc_box(7_u32);
        assert!(recovered.is_ok(), "the slot was not returned to the pool after the panic");
        assert_eq!(*recovered.unwrap(), 7);
    }

    check(|p| {
        let _ = p.try_alloc_box_with(|| -> u32 { panic!("boom") });
    });
    check(|p| {
        let _ = p.try_alloc_arc_with(|| -> u32 { panic!("boom") });
    });
    check(|p| {
        let _ = p.try_alloc_with(|| -> u32 { panic!("boom") });
    });
    check(|p| {
        let _ = p.try_alloc_rc_with(|| -> u32 { panic!("boom") });
    });
    check(|p| {
        let _ = p.try_alloc_arc_pin_with(|| -> u32 { panic!("boom") });
    });
    check(|p| {
        let _ = p.try_alloc_rc_pin_with(|| -> u32 { panic!("boom") });
    });
}

// ── reentrancy ───────────────────────────────────────────────────────────

/// A pooled value whose destructor allocates from, and frees into, the pool
/// that holds it.
///
/// Reclamation is pointer recovery and never consults the directory, so there
/// is no borrow for a destructor to re-enter.
/// Ref: docs/design/blind-pool.md, "Concurrency".
struct AllocatesOnDrop<'pool> {
    pool: &'pool BlindPool,
    /// Accumulates the value the destructor read back, so a test can tell that
    /// the nested round trip happened.
    observed: StdArc<AtomicUsize>,
}

impl Drop for AllocatesOnDrop<'_> {
    fn drop(&mut self) {
        let nested = self.pool.alloc_box(1_u8);
        self.observed.fetch_add(usize::from(*nested), Ordering::SeqCst);
    }
}

#[test]
fn a_destructor_may_allocate_from_the_pool_that_holds_it() {
    let observed = StdArc::new(AtomicUsize::new(0));
    let pool = BlindPool::new();

    // The first destructor meets `u8` for the first time, so it drives the cold
    // path from inside reclamation; the second takes the hot path.
    for expected in 1..=2 {
        let value = pool.alloc_box(AllocatesOnDrop {
            pool: &pool,
            observed: observed.clone(),
        });
        drop(value);
        assert_eq!(observed.load(Ordering::SeqCst), expected);
    }

    assert_eq!(pool.layouts(), 2);
    // Everything the destructors allocated was freed again.
    assert!(pool.is_empty());
}

#[test]
fn a_rejected_values_destructor_may_allocate_from_the_pool() {
    let observed = StdArc::new(AtomicUsize::new(0));
    let pool = BlindPool::builder().max_layouts(1).build();

    // The one permitted layout is the one the destructor allocates, so the
    // rejected value's destructor can run to completion.
    let seed = pool.alloc_box(0_u8);

    // The cap rejects this layout, and the value is dropped on the way out —
    // after every directory borrow has been released.
    // Ref: docs/implementation/blind-pool.md, "Reentrancy", step 2.
    let rejected = pool.try_alloc_box(AllocatesOnDrop {
        pool: &pool,
        observed: observed.clone(),
    });
    assert!(rejected.is_err());
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    assert_eq!(pool.layouts(), 1);
    drop(seed);
}

#[test]
fn a_construction_closure_may_allocate_from_the_same_pool() {
    let pool = BlindPool::new();
    let seed = pool.alloc_box(1_u64);

    let outer = pool.alloc_box_with(|| {
        // An already-seen layout takes the lookup path...
        let known = pool.alloc_box(2_u64);
        // ...and an unseen one drives a nested cold path while the outer
        // allocation holds a claimed slot. The directory may reallocate here;
        // the outer allocation addresses a heap `PoolInner` that never moves.
        // Ref: docs/implementation/blind-pool.md, "Reentrancy".
        let unseen = pool.alloc_box([3_u16; 3]);
        u32::try_from(*known).unwrap() + u32::from(unseen[0])
    });

    assert_eq!(*outer, 5);
    // u64 (outer seed and the seen nested value), u32 (the outer value) and
    // [u16; 3] (the unseen nested value).
    assert_eq!(pool.layouts(), 3);
    assert_eq!(pool.len(), 2);

    // The same discipline holds for the other handle flavours.
    let shared = pool.alloc_arc_with(|| {
        let nested = pool.alloc_arc(Triple(1, 2, 3));
        nested.0 + nested.1 + nested.2
    });
    assert_eq!(*shared, 6);
    let local = pool.alloc_rc_with(|| {
        let nested = pool.alloc_rc(Wide16([7; 16]));
        nested.0[0]
    });
    assert_eq!(*local, 7);

    drop((seed, outer, shared, local));
    assert!(pool.is_empty());
}

/// Allocates and frees one value of each listed array layout, all distinct.
macro_rules! alloc_each_layout {
    ($pool:expr, $($size:literal),+) => {
        $( drop($pool.alloc_box([0_u8; $size])); )+
    };
}

#[test]
fn a_construction_closure_may_grow_the_directory() {
    let pool = BlindPool::new();

    let outer = pool.alloc_box_with(|| {
        // Enough unseen layouts that the directory outgrows its buffer several
        // times over while the outer allocation is still in flight, so a
        // reference held across the closure would dangle.
        // Ref: docs/implementation/blind-pool.md, "Reentrancy".
        alloc_each_layout!(&pool, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
        99_u32
    });

    assert_eq!(*outer, 99);
    // The twelve nested layouts plus the outer `u32`.
    assert_eq!(pool.layouts(), 13);
    assert_eq!(pool.len(), 1);
    drop(outer);
    assert!(pool.is_empty());
}

/// Which layout [`ReentrantAllocator`] re-enters the pool for.
#[derive(Clone, Copy)]
enum ReentryTarget {
    /// The layout the outer allocation is building a pool for, so the outer
    /// call's re-scan finds an entry that did not exist when it started.
    SameLayout,
    /// A different layout, so the outer call's cap re-check sees the layout
    /// allowance consumed while it was building.
    OtherLayout,
}

/// State shared between [`ReentrantAllocator`] and the tests driving it.
struct ReentryState {
    /// Weak, because the pool owns the allocator that reaches back to it.
    pool: RefCell<StdWeak<BlindPool<ReentrantAllocator>>>,
    target: ReentryTarget,
    /// Set while a reentrant allocation is in flight. The nested allocation
    /// clones the allocator again, and unguarded that recurses without end.
    active: Cell<bool>,
    /// Counts reentrant allocations, so a test can tell the hook actually ran.
    reentries: Cell<u32>,
}

impl ReentryState {
    fn new(target: ReentryTarget) -> Self {
        Self {
            pool: RefCell::new(StdWeak::new()),
            target,
            active: Cell::new(false),
            reentries: Cell::new(0),
        }
    }
}

/// An allocator that allocates from the blind pool owning it while being
/// cloned.
///
/// `A::clone` runs inside the window where a new layout pool is being built,
/// which is the one reentrancy point no user-written closure can reach.
/// Ref: docs/implementation/verification.md, "Test targets".
struct ReentrantAllocator(StdRc<ReentryState>);

impl Clone for ReentrantAllocator {
    fn clone(&self) -> Self {
        let state = StdRc::clone(&self.0);
        if !state.active.replace(true) {
            state.reentries.set(state.reentries.get() + 1);
            let target = state.pool.borrow().upgrade();
            if let Some(pool) = target {
                // The values are immaterial; only their layouts decide which
                // branch of the outer call's re-scan is taken.
                match state.target {
                    ReentryTarget::SameLayout => drop(pool.alloc_box(0_u8)),
                    ReentryTarget::OtherLayout => drop(pool.alloc_box(0_u64)),
                }
            }
            state.active.set(false);
        }
        Self(state)
    }
}

// SAFETY: allocation and deallocation forward unchanged to `Global`.
unsafe impl Allocator for ReentrantAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, BackingAllocError> {
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded under the caller's `Allocator` contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

/// Builds a pool whose allocator re-enters it, wired to `state`.
fn reentrant_pool(state: &StdRc<ReentryState>, max_layouts: Option<usize>) -> StdRc<BlindPool<ReentrantAllocator>> {
    let builder = BlindPool::builder().allocator(ReentrantAllocator(StdRc::clone(state)));
    let builder = match max_layouts {
        Some(max) => builder.max_layouts(max),
        None => builder,
    };
    let pool = StdRc::new(builder.build());
    *state.pool.borrow_mut() = StdRc::downgrade(&pool);
    pool
}

#[test]
fn reentry_while_building_a_layout_pool_leaves_one_pool_per_layout() {
    let state = StdRc::new(ReentryState::new(ReentryTarget::SameLayout));
    let pool = reentrant_pool(&state, None);

    // The first sight of `u8` clones the allocator, and that clone allocates
    // the very layout being built. Without the re-scan the layout would end up
    // with two pools, one of them dead but owned.
    // Ref: docs/implementation/blind-pool.md, "Reentrancy", step 5.
    let value = pool.alloc_box(1_u8);

    assert_eq!(*value, 1);
    assert_eq!(state.reentries.get(), 1, "the allocator's Clone must have re-entered the pool");
    assert_eq!(pool.layouts(), 1, "the duplicate layout pool must be abandoned");
    assert_eq!(pool.len_of::<u8>(), 1);
    assert_eq!(pool.chunks_allocated_of::<u8>(), 1);

    drop(value);
    assert!(pool.is_empty());
}

#[test]
fn reentry_while_building_a_layout_pool_does_not_overshoot_the_layout_cap() {
    let state = StdRc::new(ReentryState::new(ReentryTarget::OtherLayout));
    let pool = reentrant_pool(&state, Some(1));

    // The reentrant miss consumes the pool's only layout allowance between the
    // outer call's two cap checks. Without the re-check both would push and the
    // cap would overshoot by the depth of reentrant misses.
    // Ref: docs/implementation/blind-pool.md, "Reentrancy", step 5.
    let Err(err) = pool.try_alloc_box(1_u8) else {
        panic!("the layout cap must reject the outer allocation");
    };

    assert!(err.is_capacity_exhausted());
    assert_eq!(state.reentries.get(), 1, "the allocator's Clone must have re-entered the pool");
    assert_eq!(pool.layouts(), 1, "the cap bounds the directory whatever the reentry depth");
    assert_eq!(pool.max_layouts(), Some(1));

    // The layout the reentrant allocation created serves ordinary requests.
    assert_eq!(*pool.alloc_box(2_u64), 2);
}

#[test]
fn reentry_that_grows_the_directory_leaves_room_for_the_outer_push() {
    let state = StdRc::new(ReentryState::new(ReentryTarget::OtherLayout));
    let pool = reentrant_pool(&state, None);

    // The reentrant miss pushes its own layout into the directory between the
    // outer call's reservation and its push. The outer push must still find the
    // room it reserved, and must not reallocate a vector the reentrant call
    // could have been holding.
    // Ref: docs/implementation/blind-pool.md, "Reentrancy", step 4.
    let value = pool.alloc_box(1_u8);

    assert_eq!(*value, 1);
    assert_eq!(state.reentries.get(), 1, "the allocator's Clone must have re-entered the pool");
    assert_eq!(pool.layouts(), 2, "both the reentrant layout and the outer one must be present");
    assert_eq!(pool.len_of::<u8>(), 1);
    assert_eq!(pool.len_of::<u64>(), 0, "the reentrant allocation was dropped again");

    // Both layouts serve further requests, so neither directory entry is stale.
    let more = (pool.alloc_box(2_u8), pool.alloc_box(3_u64));
    assert_eq!((*more.0, *more.1), (2, 3));
    assert_eq!(pool.layouts(), 2);

    drop((value, more));
    assert!(pool.is_empty());
}

// ── thread mobility ──────────────────────────────────────────────────────

/// An allocator bound to one thread, so that a pool over it is not `Send`.
#[derive(Clone)]
struct ThreadBoundAllocator(PhantomData<*const ()>);

// SAFETY: allocation and deallocation forward unchanged to `Global`.
unsafe impl Allocator for ThreadBoundAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, BackingAllocError> {
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded under the caller's `Allocator` contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

// The pool object owns no values and offers no way to reach one, so its
// `Send`-ness is a property of the allocator alone — there is no element type
// to constrain. `!Sync` is what confines the directory to one thread at a time.
// Ref: docs/design/blind-pool.md, "Concurrency".
static_assertions::assert_impl_all!(BlindPool<Global>: Send);
static_assertions::assert_not_impl_any!(BlindPool<Global>: Sync);
static_assertions::assert_not_impl_any!(BlindPool<ThreadBoundAllocator>: Send, Sync);

#[test]
fn a_thread_bound_allocator_still_serves_the_pool() {
    let pool = BlindPool::builder().allocator(ThreadBoundAllocator(PhantomData)).build();
    assert_eq!(*pool.alloc_box(5_u64), 5);
    assert_eq!(pool.alloc_box(Wide16([6; 16])).0, [6; 16]);
}

#[test]
fn a_pool_moved_to_another_thread_serves_allocations_there() {
    let pool = BlindPool::new();
    let held = pool.alloc_box(123_u32);

    let reported = thread::spawn(move || {
        let text = pool.alloc_box(String::from("there"));
        (pool.layouts(), pool.len(), text.len())
    })
    .join()
    .unwrap();

    assert_eq!(*held, 123);
    assert_eq!(reported, (2, 2, 5));
}

// ── statistics ───────────────────────────────────────────────────────────

// Blind-pool statistics live in `tests/stats.rs` alongside the typed-pool
// statistics, because the `stats` feature owns that file.
