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
#![allow(missing_docs, reason = "benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]

//! Callgrind comparison of pre-warmed allocate-and-free paths. Guard-based
//! pools receive the same payload write performed by insertion-based pools;
//! owning and borrow-bound variants are measured separately.

use std::hint::black_box;

use gungraun::prelude::*;
use plurality::Pool;

/// Iterations of allocate+free per benchmark body.
const COUNT: u64 = 10_000;
/// Number of slots each pool is pre-warmed with.
const CAP: usize = 1024;

/// A `Drop`-free payload that isolates pool allocation costs.
#[derive(Clone)]
#[allow(dead_code, reason = "fields set a realistic object size for the benchmark")]
struct Obj {
    a: u64,
    b: [u64; 3],
}

impl Obj {
    #[inline]
    fn new(i: u64) -> Self {
        Self {
            a: i,
            b: [i, i ^ 0xFF, i.wrapping_mul(0x9E37_79B9)],
        }
    }
}

// ---------------------------------------------------------------------------
// plurality
// ---------------------------------------------------------------------------

fn setup_plurality(n: usize) -> Pool<Obj> {
    let pool = Pool::<Obj>::builder().chunk_size(CAP as u32).build();
    let warm: Vec<_> = (0..n).map(|i| pool.alloc_box(Obj::new(i as u64))).collect();
    drop(warm);
    pool
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_plurality)]
fn plurality_box(pool: Pool<Obj>) -> Pool<Obj> {
    for i in 0..COUNT {
        let h = pool.alloc_box(black_box(Obj::new(i)));
        drop(black_box(h));
    }
    pool
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_plurality)]
fn plurality_alloc(pool: Pool<Obj>) -> Pool<Obj> {
    for i in 0..COUNT {
        let h = pool.alloc(black_box(Obj::new(i)));
        drop(black_box(h));
    }
    pool
}

// ---------------------------------------------------------------------------
// slab (index-based, single-thread)
// ---------------------------------------------------------------------------

fn setup_slab(n: usize) -> slab::Slab<Obj> {
    let mut slab = slab::Slab::with_capacity(n);
    let keys: Vec<_> = (0..n).map(|i| slab.insert(Obj::new(i as u64))).collect();
    for k in keys {
        slab.remove(k);
    }
    slab
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_slab)]
fn slab_insert_remove(mut slab: slab::Slab<Obj>) -> slab::Slab<Obj> {
    for i in 0..COUNT {
        let key = slab.insert(black_box(Obj::new(i)));
        slab.remove(black_box(key));
    }
    slab
}

// ---------------------------------------------------------------------------
// sharded-slab (lock-free, concurrent)
// ---------------------------------------------------------------------------

fn setup_sharded_slab(n: usize) -> sharded_slab::Slab<Obj> {
    let slab = sharded_slab::Slab::new();
    let keys: Vec<_> = (0..n).map(|i| slab.insert(Obj::new(i as u64)).unwrap()).collect();
    for k in keys {
        slab.remove(k);
    }
    slab
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_sharded_slab)]
fn sharded_slab_insert_remove(slab: sharded_slab::Slab<Obj>) -> sharded_slab::Slab<Obj> {
    for i in 0..COUNT {
        let key = slab.insert(black_box(Obj::new(i))).unwrap();
        slab.remove(black_box(key));
    }
    slab
}

// ---------------------------------------------------------------------------
// slotmap (generational keys, single-thread)
// ---------------------------------------------------------------------------

fn setup_slotmap(n: usize) -> slotmap::SlotMap<slotmap::DefaultKey, Obj> {
    let mut sm = slotmap::SlotMap::with_capacity(n);
    let keys: Vec<_> = (0..n).map(|i| sm.insert(Obj::new(i as u64))).collect();
    for k in keys {
        sm.remove(k);
    }
    sm
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_slotmap)]
fn slotmap_insert_remove(mut sm: slotmap::SlotMap<slotmap::DefaultKey, Obj>) -> slotmap::SlotMap<slotmap::DefaultKey, Obj> {
    for i in 0..COUNT {
        let key = sm.insert(black_box(Obj::new(i)));
        sm.remove(black_box(key));
    }
    sm
}

// ---------------------------------------------------------------------------
// object-pool (RAII guard, spin-lock)
// ---------------------------------------------------------------------------

fn setup_object_pool(n: usize) -> object_pool::Pool<Obj> {
    object_pool::Pool::new(n, || Obj::new(0))
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_object_pool)]
fn object_pool_pull(pool: object_pool::Pool<Obj>) -> object_pool::Pool<Obj> {
    for i in 0..COUNT {
        let mut guard = pool.try_pull().unwrap();
        *guard = black_box(Obj::new(i));
        drop(black_box(guard));
    }
    pool
}

// ---------------------------------------------------------------------------
// opool (lock-free RAII guard)
// ---------------------------------------------------------------------------

struct ObjAllocator;

impl opool::PoolAllocator<Obj> for ObjAllocator {
    #[inline]
    fn allocate(&self) -> Obj {
        Obj::new(0)
    }
}

fn setup_opool(n: usize) -> opool::Pool<ObjAllocator, Obj> {
    opool::Pool::new_prefilled(n, ObjAllocator)
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_opool)]
fn opool_get(pool: opool::Pool<ObjAllocator, Obj>) -> opool::Pool<ObjAllocator, Obj> {
    for i in 0..COUNT {
        let mut guard = pool.get();
        *guard = black_box(Obj::new(i));
        drop(black_box(guard));
    }
    pool
}

// ---------------------------------------------------------------------------
// deadpool (unmanaged; async pool driven synchronously via try_get)
// ---------------------------------------------------------------------------

fn setup_deadpool(n: usize) -> deadpool::unmanaged::Pool<Obj> {
    deadpool::unmanaged::Pool::from((0..n).map(|i| Obj::new(i as u64)).collect::<Vec<_>>())
}

#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_deadpool)]
fn deadpool_get(pool: deadpool::unmanaged::Pool<Obj>) -> deadpool::unmanaged::Pool<Obj> {
    for i in 0..COUNT {
        let mut guard = pool.try_get().unwrap();
        *guard = black_box(Obj::new(i));
        drop(black_box(guard));
    }
    pool
}

// ---------------------------------------------------------------------------
// infinity_pool (pinned pool; refcounted and raw access models)
// ---------------------------------------------------------------------------

fn setup_infinity_pinned(n: usize) -> infinity_pool::PinnedPool<Obj> {
    let pool = infinity_pool::PinnedPool::<Obj>::new();
    pool.reserve(n);
    let warm: Vec<_> = (0..n).map(|i| pool.insert(Obj::new(i as u64))).collect();
    drop(warm);
    pool
}

// Thread-safe, reference-counted handle (`Arc` style) — the fair analog to
// `plurality_box`.
#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_infinity_pinned)]
fn infinity_pinned(pool: infinity_pool::PinnedPool<Obj>) -> infinity_pool::PinnedPool<Obj> {
    for i in 0..COUNT {
        let h = pool.insert(black_box(Obj::new(i)));
        drop(black_box(h));
    }
    pool
}

fn setup_infinity_raw(n: usize) -> infinity_pool::RawPinnedPool<Obj> {
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

// Raw access model with no reference counting (manual lifetime management) —
// the fair analog to `plurality_alloc`.
#[library_benchmark]
#[bench::churn(args = (CAP,), setup = setup_infinity_raw)]
fn infinity_raw(mut pool: infinity_pool::RawPinnedPool<Obj>) -> infinity_pool::RawPinnedPool<Obj> {
    for i in 0..COUNT {
        let h = pool.insert(black_box(Obj::new(i)));
        // SAFETY: `h` was just returned by this pool's `insert` and is removed exactly once.
        unsafe {
            pool.remove(black_box(h));
        }
    }
    pool
}

library_benchmark_group!(
    name = comparison,
    benchmarks = [
        plurality_box,
        plurality_alloc,
        slab_insert_remove,
        sharded_slab_insert_remove,
        slotmap_insert_remove,
        object_pool_pull,
        opool_get,
        deadpool_get,
        infinity_pinned,
        infinity_raw
    ]
);
