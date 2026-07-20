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

//! Single-operation benchmark bodies for the gungraun (Callgrind) alloc and
//! fat-pointer suites.
//!
//! Each `fn` performs **exactly one** hot operation, run once so Callgrind can
//! count its instructions exactly. The criterion suite has its own copy of these
//! bodies (`benches/criterion/ops.rs`) that it loops for a wall-clock signal.

use std::hint::black_box;

use infinity_pool::{BlindPool, LocalBlindPool, LocalPinnedPool, PinnedPool, define_pooled_dyn_cast};
use plurality::{Arc, Pool, Rc, coerce};

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
    let d: plurality::Arc<dyn Marker> = plurality::Arc::unsize::<dyn Marker>(a, plurality::coerce!(dyn Marker));
    drop(black_box(d));
}

#[inline]
pub(crate) fn box_unsize(p: &Pool<Obj>, i: u64) {
    let b = p.alloc_box(black_box(Obj::new(i)));
    let d: plurality::Box<dyn Marker> = plurality::Box::unsize::<dyn Marker>(b, plurality::coerce!(dyn Marker));
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
    let handle: plurality::Box<dyn Marker> = plurality::Box::unsize(handle, coerce!(dyn Marker));
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
    let warm: Vec<std::boxed::Box<dyn Marker>> = (0..n)
        .map(|i| std::boxed::Box::new(Obj::new(i as u64)) as std::boxed::Box<dyn Marker>)
        .collect();
    black_box(&warm);
    drop(warm);
    let handle: std::boxed::Box<dyn Marker> = std::boxed::Box::new(Obj::new(n as u64));
    assert_eq!(black_box::<&dyn Marker>(&*handle).tag(), 0xFF);
    drop(black_box(handle));
}

// ── fat-pointer comparison bodies ────────────────────────────────────────

#[inline]
pub(crate) fn plurality_box(pool: &Pool<Obj>, i: u64) {
    let handle = pool.alloc_box(black_box(Obj::new(i)));
    let handle: plurality::Box<dyn Marker> = plurality::Box::unsize(handle, coerce!(dyn Marker));
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
    let handle: std::boxed::Box<dyn Marker> = std::boxed::Box::new(black_box(Obj::new(i)));
    invoke_dyn(&*handle);
    drop(black_box(handle));
}
