// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    reason = "benchmark code"
)]

//! Single allocate-and-free bodies shared by the Criterion and Callgrind
//! cross-crate pool comparisons.
//!
//! Guard-based pools receive the same payload write performed by
//! insertion-based pools, so every body performs one acquire, one payload
//! initialization, and one release against a pre-warmed pool.

use std::hint::black_box;

use plurality::Pool;

/// Slots each pool is pre-warmed with, so a measured body only ever reuses a
/// slot and never grows the pool.
pub(crate) const CAP: usize = 1024;

/// A `Drop`-free payload that isolates pool allocation costs.
#[derive(Clone)]
#[allow(dead_code, reason = "fields set a realistic object size for the benchmark")]
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

// ---------------------------------------------------------------------------
// plurality
// ---------------------------------------------------------------------------

pub(crate) fn setup_plurality(n: usize) -> Pool<Obj> {
    let pool = Pool::<Obj>::builder().chunk_size(CAP as u32).build();
    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    pool
}

#[inline]
pub(crate) fn plurality_box(pool: &Pool<Obj>, i: u64) {
    let handle = pool.alloc_box(black_box(Obj::new(i)));
    drop(black_box(handle));
}

#[inline]
pub(crate) fn plurality_alloc(pool: &Pool<Obj>, i: u64) {
    let handle = pool.alloc(black_box(Obj::new(i)));
    drop(black_box(handle));
}

// ---------------------------------------------------------------------------
// slab (index-based, single-thread)
// ---------------------------------------------------------------------------

pub(crate) fn setup_slab(n: usize) -> slab::Slab<Obj> {
    let mut slab = slab::Slab::with_capacity(n);
    let keys: Vec<_> = (0..n).map(|i| slab.insert(Obj::new(i as u64))).collect();
    for k in keys {
        slab.remove(k);
    }
    slab
}

#[inline]
pub(crate) fn slab_insert_remove(slab: &mut slab::Slab<Obj>, i: u64) {
    let key = slab.insert(black_box(Obj::new(i)));
    black_box(slab.remove(black_box(key)));
}

// ---------------------------------------------------------------------------
// sharded-slab (lock-free, concurrent)
// ---------------------------------------------------------------------------

pub(crate) fn setup_sharded_slab(n: usize) -> sharded_slab::Slab<Obj> {
    let slab = sharded_slab::Slab::new();
    let keys: Vec<_> = (0..n).map(|i| slab.insert(Obj::new(i as u64)).unwrap()).collect();
    for k in keys {
        slab.remove(k);
    }
    slab
}

#[inline]
pub(crate) fn sharded_slab_insert_remove(slab: &sharded_slab::Slab<Obj>, i: u64) {
    let key = slab.insert(black_box(Obj::new(i))).unwrap();
    black_box(slab.remove(black_box(key)));
}

// ---------------------------------------------------------------------------
// slotmap (generational keys, single-thread)
// ---------------------------------------------------------------------------

pub(crate) fn setup_slotmap(n: usize) -> slotmap::SlotMap<slotmap::DefaultKey, Obj> {
    let mut sm = slotmap::SlotMap::with_capacity(n);
    let keys: Vec<_> = (0..n).map(|i| sm.insert(Obj::new(i as u64))).collect();
    for k in keys {
        sm.remove(k);
    }
    sm
}

#[inline]
pub(crate) fn slotmap_insert_remove(sm: &mut slotmap::SlotMap<slotmap::DefaultKey, Obj>, i: u64) {
    let key = sm.insert(black_box(Obj::new(i)));
    black_box(sm.remove(black_box(key)));
}

// ---------------------------------------------------------------------------
// object-pool (RAII guard, spin-lock)
// ---------------------------------------------------------------------------

pub(crate) fn setup_object_pool(n: usize) -> object_pool::Pool<Obj> {
    object_pool::Pool::new(n, || Obj::new(0))
}

#[inline]
pub(crate) fn object_pool_pull(pool: &object_pool::Pool<Obj>, i: u64) {
    let mut guard = pool.try_pull().unwrap();
    *guard = black_box(Obj::new(i));
    drop(black_box(guard));
}

// ---------------------------------------------------------------------------
// opool (lock-free RAII guard)
// ---------------------------------------------------------------------------

pub(crate) struct ObjAllocator;

impl opool::PoolAllocator<Obj> for ObjAllocator {
    #[inline]
    fn allocate(&self) -> Obj {
        Obj::new(0)
    }
}

pub(crate) fn setup_opool(n: usize) -> opool::Pool<ObjAllocator, Obj> {
    opool::Pool::new_prefilled(n, ObjAllocator)
}

#[inline]
pub(crate) fn opool_get(pool: &opool::Pool<ObjAllocator, Obj>, i: u64) {
    let mut guard = pool.get();
    *guard = black_box(Obj::new(i));
    drop(black_box(guard));
}

// ---------------------------------------------------------------------------
// deadpool (unmanaged; async pool driven synchronously via try_get)
// ---------------------------------------------------------------------------

pub(crate) fn setup_deadpool(n: usize) -> deadpool::unmanaged::Pool<Obj> {
    deadpool::unmanaged::Pool::from((0..n).map(|i| Obj::new(i as u64)).collect::<Vec<_>>())
}

#[inline]
pub(crate) fn deadpool_get(pool: &deadpool::unmanaged::Pool<Obj>, i: u64) {
    let mut guard = pool.try_get().unwrap();
    *guard = black_box(Obj::new(i));
    drop(black_box(guard));
}

// ---------------------------------------------------------------------------
// infinity_pool (pinned pool; refcounted and raw access models)
// ---------------------------------------------------------------------------

pub(crate) fn setup_infinity_pinned(n: usize) -> infinity_pool::PinnedPool<Obj> {
    let pool = infinity_pool::PinnedPool::<Obj>::new();
    pool.reserve(n);
    let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    drop(warm);
    pool
}

/// Thread-safe, reference-counted handle (`Arc` style) — the fair analog to
/// [`plurality_box`].
#[inline]
pub(crate) fn infinity_pinned(pool: &infinity_pool::PinnedPool<Obj>, i: u64) {
    let handle = pool.insert(black_box(Obj::new(i)));
    drop(black_box(handle));
}

pub(crate) fn setup_infinity_raw(n: usize) -> infinity_pool::RawPinnedPool<Obj> {
    let mut pool = infinity_pool::RawPinnedPool::<Obj>::new();
    let handles: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    for h in handles {
        // SAFETY: each handle was just returned by this pool's `insert` and is removed exactly once.
        unsafe {
            pool.remove(h);
        }
    }
    pool
}

/// Raw access model with no reference counting (manual lifetime management) —
/// the fair analog to [`plurality_alloc`].
#[inline]
pub(crate) fn infinity_raw(pool: &mut infinity_pool::RawPinnedPool<Obj>, i: u64) {
    let handle = pool.insert(black_box(Obj::new(i)));
    // SAFETY: `handle` was just returned by this pool's `insert` and is removed exactly once.
    unsafe {
        pool.remove(black_box(handle));
    }
}
