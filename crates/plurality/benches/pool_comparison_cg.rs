// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(missing_docs, reason = "benchmark code")]
#![cfg_attr(
    target_os = "linux",
    allow(
        clippy::allow_attributes,
        clippy::exit,
        clippy::missing_docs_in_private_items,
        clippy::needless_pass_by_value,
        unused_qualifications,
        reason = "triggered by the gungraun main! macro expansion and its by-value bench inputs"
    )
)]

//! Callgrind comparison of pre-warmed allocate-and-free paths across the
//! pooling crates plurality competes with.
//!
//! Paired with `pool_comparison.rs`, which measures the same bodies under
//! Criterion. gungraun needs Valgrind (Linux-only); on other targets this bench
//! compiles to a no-op.

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
#[path = "pool_comparison/shared.rs"]
mod shared;

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::{library_benchmark, library_benchmark_group};

    use crate::shared::{
        self, CAP, Obj, ObjAllocator, setup_deadpool, setup_infinity_pinned, setup_infinity_raw, setup_object_pool, setup_opool,
        setup_plurality, setup_sharded_slab, setup_slab, setup_slotmap,
    };

    /// Iterations of allocate+free per benchmark body.
    const COUNT: u64 = 10_000;

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_plurality)]
    fn churn_plurality_box(pool: plurality::Pool<Obj>) -> plurality::Pool<Obj> {
        for i in 0..COUNT {
            shared::plurality_box(&pool, i);
        }
        pool
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_plurality)]
    fn churn_plurality_alloc(pool: plurality::Pool<Obj>) -> plurality::Pool<Obj> {
        for i in 0..COUNT {
            shared::plurality_alloc(&pool, i);
        }
        pool
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_slab)]
    fn churn_slab_insert_remove(mut slab: slab::Slab<Obj>) -> slab::Slab<Obj> {
        for i in 0..COUNT {
            shared::slab_insert_remove(&mut slab, i);
        }
        slab
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_sharded_slab)]
    fn churn_sharded_slab_insert_remove(slab: sharded_slab::Slab<Obj>) -> sharded_slab::Slab<Obj> {
        for i in 0..COUNT {
            shared::sharded_slab_insert_remove(&slab, i);
        }
        slab
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_slotmap)]
    fn churn_slotmap_insert_remove(mut sm: slotmap::SlotMap<slotmap::DefaultKey, Obj>) -> slotmap::SlotMap<slotmap::DefaultKey, Obj> {
        for i in 0..COUNT {
            shared::slotmap_insert_remove(&mut sm, i);
        }
        sm
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_object_pool)]
    fn churn_object_pool_pull(pool: object_pool::Pool<Obj>) -> object_pool::Pool<Obj> {
        for i in 0..COUNT {
            shared::object_pool_pull(&pool, i);
        }
        pool
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_opool)]
    fn churn_opool_get(pool: opool::Pool<ObjAllocator, Obj>) -> opool::Pool<ObjAllocator, Obj> {
        for i in 0..COUNT {
            shared::opool_get(&pool, i);
        }
        pool
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_deadpool)]
    fn churn_deadpool_get(pool: deadpool::unmanaged::Pool<Obj>) -> deadpool::unmanaged::Pool<Obj> {
        for i in 0..COUNT {
            shared::deadpool_get(&pool, i);
        }
        pool
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_infinity_pinned)]
    fn churn_infinity_pinned(pool: infinity_pool::PinnedPool<Obj>) -> infinity_pool::PinnedPool<Obj> {
        for i in 0..COUNT {
            shared::infinity_pinned(&pool, i);
        }
        pool
    }

    #[library_benchmark]
    #[bench::churn(args = (CAP,), setup = setup_infinity_raw)]
    fn churn_infinity_raw(mut pool: infinity_pool::RawPinnedPool<Obj>) -> infinity_pool::RawPinnedPool<Obj> {
        for i in 0..COUNT {
            shared::infinity_raw(&mut pool, i);
        }
        pool
    }

    library_benchmark_group!(
        name = churn;
        benchmarks =
            churn_plurality_box,
            churn_plurality_alloc,
            churn_slab_insert_remove,
            churn_sharded_slab_insert_remove,
            churn_slotmap_insert_remove,
            churn_object_pool_pull,
            churn_opool_get,
            churn_deadpool_get,
            churn_infinity_pinned,
            churn_infinity_raw
    );
}

#[cfg(target_os = "linux")]
use linux::churn;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = churn);
