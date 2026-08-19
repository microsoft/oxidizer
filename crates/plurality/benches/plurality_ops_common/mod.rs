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

//! Shared setup and operation bodies for allocation and fat-pointer benchmarks.

use std::boxed::Box as StdBox;
use std::hint::black_box;

use infinity_pool::{BlindPool, LocalBlindPool, LocalPinnedPool, PinnedPool, define_pooled_dyn_cast};
use plurality::{Arc, Box as PoolBox, MultiPool, Pool, Rc, coerce};

mod metadata;

pub(crate) use metadata::SPREAD_LAYOUTS;

/// A small (~32-byte), `Drop`-free payload, so the benchmarks measure the
/// pool's own allocate/free cost rather than user destructors.
#[derive(Clone, Debug)]
pub(crate) struct Obj {
    a: u64,
    b: [u64; 3],
}

impl Obj {
    #[inline]
    pub(crate) fn new(i: u64) -> Self {
        Self {
            a: i,
            b: [i, i ^ 0xFF, i.wrapping_mul(0x9E37_79B9)],
        }
    }
}

/// A trivial trait so an `Obj` handle can be erased to `dyn Marker`, exercising
/// the erased (fat-pointer) allocate/free path. `tag()` is defined so the
/// sentinel object built at index [`CAP`] hashes to `0xFF`.
pub(crate) trait Marker {
    fn tag(&self) -> u64;
}

impl Marker for Obj {
    #[inline]
    fn tag(&self) -> u64 {
        self.a ^ self.b[1]
    }
}

define_pooled_dyn_cast!(Marker);

#[inline]
fn invoke_dyn(value: &dyn Marker) {
    black_box(black_box(value).tag());
}

/// Slots to pre-warm with (and the chunk size), so the timed region only ever
/// reuses slots and never grows a chunk.
pub(crate) const CAP: usize = 1024;

// ── alloc setup ──────────────────────────────────────────────────────────

/// A pool pre-warmed with `n` reusable slots.
pub(crate) fn setup_pool(n: usize) -> Pool<Obj> {
    let pool = Pool::<Obj>::builder().chunk_size(CAP as u32).build();
    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    pool
}

/// A pre-warmed pool plus a live `Arc` to clone in the clone benchmark.
pub(crate) fn setup_arc(n: usize) -> (Pool<Obj>, Arc<Obj>) {
    let pool = setup_pool(n);
    let base = pool.alloc_arc(Obj::new(0));
    (pool, base)
}

/// A pre-warmed pool plus a live `Rc` to clone in the clone benchmark.
pub(crate) fn setup_rc(n: usize) -> (Pool<Obj>, Rc<Obj>) {
    let pool = setup_pool(n);
    let base = pool.alloc_rc(Obj::new(0));
    (pool, base)
}

/// A multi pool serving a single layout, pre-warmed exactly as [`setup_pool`]
/// warms the typed pool.
pub(crate) fn setup_multi_pool(n: usize) -> MultiPool {
    let pool = MultiPool::builder().chunk_size(CAP as u32).build();
    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    pool
}

/// A multi pool serving [`SPREAD_LAYOUTS`] layouts, with `Obj` registered last.
///
/// Directory entries are held in first-seen order, so registering `Obj` after
/// the fillers makes the measured allocation traverse the whole key vector.
/// Ref: docs/implementation/multi-pool.md, "Lookup".
pub(crate) fn setup_multi_pool_spread(n: usize) -> MultiPool {
    let pool = MultiPool::builder().chunk_size(CAP as u32).build();

    /// Registers one filler layout per byte length. Each length routes to a
    /// pool of its own, and none of them collides with `Obj`, whose size is
    /// larger than any of them.
    macro_rules! fillers {
        ($($len:literal),*) => {
            $( drop(pool.alloc_box([0_u8; $len])); )*
        };
    }
    fillers!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    assert_eq!(pool.layouts(), SPREAD_LAYOUTS - 1);

    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    assert_eq!(pool.layouts(), SPREAD_LAYOUTS);
    pool
}

/// A multi pool holding one layout that `Obj` does not route to, so allocating
/// an `Obj` misses the directory and installs a layout pool.
///
/// The directory is one entry long, as in [`setup_multi_pool`], so the scan the
/// measured allocation runs is the same length in both and the rows differ only
/// in whether it finds its layout. `n` sets the chunk size, because the chunk
/// the install grows is part of what a miss allocates.
pub(crate) fn setup_multi_pool_miss(n: usize) -> MultiPool {
    let pool = MultiPool::builder().chunk_size(n as u32).build();

    // A routing key pairs size with widened alignment, so a one-byte filler
    // cannot collide with the larger `Obj`.
    // Ref: docs/implementation/multi-pool.md, "Lookup".
    drop(pool.alloc_box(0_u8));
    assert_eq!(pool.layouts(), 1);

    pool
}

// ── Box ──────────────────────────────────────────────────────────────────

#[inline]
pub(crate) fn box_val(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_box(black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn box_with(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_box_with(|| black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn box_uninit(p: &Pool<Obj>, i: u64) {
    let mut u = p.alloc_uninit_box();
    u.write(black_box(Obj::new(i)));
    // SAFETY: the value was just written.
    drop(black_box(unsafe { u.assume_init() }));
}

#[inline]
pub(crate) fn arc_unsize(p: &Pool<Obj>, i: u64) {
    let a = p.alloc_arc(black_box(Obj::new(i)));
    let d: Arc<dyn Marker> = Arc::unsize::<dyn Marker>(a, coerce!(dyn Marker));
    drop(black_box(d));
}

#[inline]
pub(crate) fn box_unsize(p: &Pool<Obj>, i: u64) {
    let b = p.alloc_box(black_box(Obj::new(i)));
    let d: PoolBox<dyn Marker> = PoolBox::unsize::<dyn Marker>(b, coerce!(dyn Marker));
    drop(black_box(d));
}

// ── Arc ──────────────────────────────────────────────────────────────────

#[inline]
pub(crate) fn arc_val(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_arc(black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn arc_with(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_arc_with(|| black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn arc_uninit(p: &Pool<Obj>, i: u64) {
    let mut u = p.alloc_uninit_arc();
    Arc::get_mut(&mut u).unwrap().write(black_box(Obj::new(i)));
    // SAFETY: the value was just written.
    drop(black_box(unsafe { u.assume_init() }));
}

// ── Alloc (lifetime-bound) ───────────────────────────────────────────────

#[inline]
pub(crate) fn alloc_val(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc(black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn alloc_with(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_with(|| black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn alloc_uninit(p: &Pool<Obj>, i: u64) {
    let mut u = p.alloc_uninit();
    u.write(black_box(Obj::new(i)));
    // SAFETY: the value was just written.
    drop(black_box(unsafe { u.assume_init() }));
}

// ── Rc ───────────────────────────────────────────────────────────────────

#[inline]
pub(crate) fn rc_val(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_rc(black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn rc_with(p: &Pool<Obj>, i: u64) {
    drop(black_box(p.alloc_rc_with(|| black_box(Obj::new(i)))));
}

#[inline]
pub(crate) fn rc_uninit(p: &Pool<Obj>, i: u64) {
    let mut u = p.alloc_uninit_rc();
    Rc::get_mut(&mut u).unwrap().write(black_box(Obj::new(i)));
    // SAFETY: the value was just written.
    drop(black_box(unsafe { u.assume_init() }));
}

// ── MultiPool (routed allocation) ────────────────────────────────────────
//
// The first two bodies below turn the typed `box_val` row into a three-rung
// ladder: typed, routed with one layout, routed with a full directory. The
// first step isolates the runtime slot stride plus a one-entry scan, the second
// gives the per-entry scan slope, which a single routed row would report as one
// summed number. The third takes the other endpoint of the routing branch, so
// the miss is covered as well as the hit.
// Ref: docs/implementation/multi-pool.md, "Lookup";
// docs/callgrind-benchmarks.md, "Scenario selection".

#[inline]
pub(crate) fn multi_box_val(p: &MultiPool, i: u64) {
    drop(black_box(p.alloc_box(black_box(Obj::new(i)))));
}

/// The body of [`multi_box_val`], measured against a pool whose directory holds
/// [`SPREAD_LAYOUTS`] layouts, so the two rows differ only in scan length.
#[inline]
pub(crate) fn multi_box_val_spread(p: &MultiPool, i: u64) {
    multi_box_val(p, i);
}

/// The body of [`multi_box_val`], measured against a pool that has never seen
/// `Obj`, so the scan misses and the layout pool is installed.
///
/// The row prices the whole first touch: the failed scan, the layout pool the
/// miss builds and installs, and the first chunk the growth path then
/// allocates. That is what presenting a new layout costs a caller, and the row
/// is not a measurement of the scan alone.
#[inline]
pub(crate) fn multi_box_val_miss(p: &MultiPool, i: u64) {
    multi_box_val(p, i);
}

// ── clone + drop (shared handles) ────────────────────────────────────────

#[inline]
pub(crate) fn arc_clone(base: &Arc<Obj>) {
    drop(black_box(base.clone()));
}

#[inline]
pub(crate) fn rc_clone(base: &Rc<Obj>) {
    drop(black_box(base.clone()));
}

// ── fat-pointer comparison setup ─────────────────────────────────────────

pub(crate) fn setup_plurality(n: usize) -> Pool<Obj> {
    let pool = Pool::<Obj>::new();
    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    assert!(pool.capacity() >= n as u64);
    assert!(pool.is_empty());
    let handle = pool.alloc_box(Obj::new(n as u64));
    let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
    assert_eq!(handle.tag(), 0xFF);
    drop(handle);
    pool
}

pub(crate) fn setup_plurality_multi(n: usize) -> MultiPool {
    let pool = MultiPool::new();
    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    assert!(pool.capacity_of::<Obj>() >= n as u64);
    assert!(pool.is_empty());
    let handle = pool.alloc_box(Obj::new(n as u64));
    let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
    assert_eq!(handle.tag(), 0xFF);
    drop(handle);
    pool
}

pub(crate) fn setup_infinity_pinned(n: usize) -> PinnedPool<Obj> {
    let pool = PinnedPool::new();
    pool.reserve(n);
    let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    drop(warm);
    assert!(pool.capacity() >= n);
    assert!(pool.is_empty());
    let handle = pool.insert(Obj::new(n as u64)).cast_marker();
    assert_eq!(handle.tag(), 0xFF);
    drop(handle);
    pool
}

pub(crate) fn setup_infinity_local_pinned(n: usize) -> LocalPinnedPool<Obj> {
    let pool = LocalPinnedPool::new();
    pool.reserve(n);
    let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    drop(warm);
    assert!(pool.capacity() >= n);
    assert!(pool.is_empty());
    let handle = pool.insert(Obj::new(n as u64)).cast_marker();
    assert_eq!(handle.tag(), 0xFF);
    drop(handle);
    pool
}

pub(crate) fn setup_infinity_blind(n: usize) -> BlindPool {
    let pool = BlindPool::new();
    pool.reserve_for::<Obj>(n);
    let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    drop(warm);
    assert!(pool.capacity_for::<Obj>() >= n);
    assert!(pool.is_empty());
    let handle = pool.insert(Obj::new(n as u64)).cast_marker();
    assert_eq!(handle.tag(), 0xFF);
    drop(handle);
    pool
}

pub(crate) fn setup_infinity_local_blind(n: usize) -> LocalBlindPool {
    let pool = LocalBlindPool::new();
    pool.reserve_for::<Obj>(n);
    let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    drop(warm);
    assert!(pool.capacity_for::<Obj>() >= n);
    assert!(pool.is_empty());
    let handle = pool.insert(Obj::new(n as u64)).cast_marker();
    assert_eq!(handle.tag(), 0xFF);
    drop(handle);
    pool
}

pub(crate) fn setup_std_box(n: usize) {
    let warm: Vec<StdBox<dyn Marker>> = (0..n).map(|i| StdBox::new(Obj::new(i as u64)) as StdBox<dyn Marker>).collect();
    black_box(&warm);
    drop(warm);
    let handle: StdBox<dyn Marker> = StdBox::new(Obj::new(n as u64));
    assert_eq!(black_box::<&dyn Marker>(&*handle).tag(), 0xFF);
    drop(black_box(handle));
}

// ── fat-pointer comparison bodies ────────────────────────────────────────

#[inline]
pub(crate) fn plurality_box(pool: &Pool<Obj>, i: u64) {
    let handle = pool.alloc_box(black_box(Obj::new(i)));
    let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
    invoke_dyn(&*handle);
    drop(black_box(handle));
}

#[inline]
pub(crate) fn plurality_multi_box(pool: &MultiPool, i: u64) {
    let handle = pool.alloc_box(black_box(Obj::new(i)));
    let handle: PoolBox<dyn Marker> = PoolBox::unsize(handle, coerce!(dyn Marker));
    invoke_dyn(&*handle);
    drop(black_box(handle));
}

#[inline]
pub(crate) fn infinity_pinned(pool: &PinnedPool<Obj>, i: u64) {
    let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
    invoke_dyn(&*handle);
    drop(black_box(handle));
}

#[inline]
pub(crate) fn infinity_local_pinned(pool: &LocalPinnedPool<Obj>, i: u64) {
    let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
    invoke_dyn(&*handle);
    drop(black_box(handle));
}

#[inline]
pub(crate) fn infinity_blind(pool: &BlindPool, i: u64) {
    let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
    invoke_dyn(&*handle);
    drop(black_box(handle));
}

#[inline]
pub(crate) fn infinity_local_blind(pool: &LocalBlindPool, i: u64) {
    let handle = pool.insert(black_box(Obj::new(i))).cast_marker();
    invoke_dyn(&*handle);
    drop(black_box(handle));
}

#[inline]
pub(crate) fn std_box(i: u64) {
    let handle: StdBox<dyn Marker> = StdBox::new(black_box(Obj::new(i)));
    invoke_dyn(&*handle);
    drop(black_box(handle));
}
