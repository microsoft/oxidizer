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

//! Shared single-operation benchmark bodies for the criterion and gungraun
//! alloc suites.
//!
//! Each `fn` here performs **exactly one** hot operation. The gungraun suite
//! (`gungraun_alloc.rs`) runs each one once — Callgrind counts instructions
//! exactly — while the criterion suite (`criterion_alloc.rs`) loops each one
//! `N` times for a wall-clock signal. Both suites call these same functions, so
//! the two measurements are guaranteed to exercise identical code.
//!
//! Included into each bench target via `#[path = "shared/ops.rs"] mod ops;`.

#![allow(dead_code, reason = "each bench target uses a subset of these helpers")]

use std::hint::black_box;

use plurality::{Arc, Pool, Rc};

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

/// Slots to pre-warm with (and the chunk size), so the timed region only ever
/// reuses slots and never grows a chunk.
pub(crate) const CAP: usize = 1024;

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
