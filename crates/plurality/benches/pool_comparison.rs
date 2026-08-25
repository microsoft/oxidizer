// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::missing_panics_doc,
    missing_debug_implementations,
    missing_docs,
    reason = "benchmark code"
)]

//! Criterion wall-clock comparison of pre-warmed allocate-and-free paths across
//! the pooling crates plurality competes with.
//!
//! Paired with `pool_comparison_cg.rs`, which measures the same bodies under
//! Callgrind. Reported iterations contain `N` operations; `perf_report.rs`
//! converts them to per-operation times.
//!
//! The competing crates are Linux-only dev-dependencies, so this bench compiles
//! to a no-op elsewhere.

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
#[path = "pool_comparison/shared.rs"]
mod shared;

#[cfg(target_os = "linux")]
mod bench {
    use std::hint::black_box;

    use criterion::Criterion;

    use crate::shared;

    /// Operations performed per criterion iteration, scaled up so wall-clock
    /// timing has signal. Mirrors the "run once" of the Callgrind suite.
    const N: u64 = 1000;

    pub(crate) fn churn(criterion: &mut Criterion) {
        let mut group = criterion.benchmark_group("pool_comparison/churn");

        let plurality = shared::setup_plurality(shared::CAP);
        group.bench_function("plurality_box", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::plurality_box(black_box(&plurality), i);
                }
            });
        });
        group.bench_function("plurality_alloc", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::plurality_alloc(black_box(&plurality), i);
                }
            });
        });

        let mut slab = shared::setup_slab(shared::CAP);
        group.bench_function("slab_insert_remove", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::slab_insert_remove(black_box(&mut slab), i);
                }
            });
        });

        let sharded = shared::setup_sharded_slab(shared::CAP);
        group.bench_function("sharded_slab_insert_remove", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::sharded_slab_insert_remove(black_box(&sharded), i);
                }
            });
        });

        let mut slotmap = shared::setup_slotmap(shared::CAP);
        group.bench_function("slotmap_insert_remove", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::slotmap_insert_remove(black_box(&mut slotmap), i);
                }
            });
        });

        let object_pool = shared::setup_object_pool(shared::CAP);
        group.bench_function("object_pool_pull", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::object_pool_pull(black_box(&object_pool), i);
                }
            });
        });

        let opool = shared::setup_opool(shared::CAP);
        group.bench_function("opool_get", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::opool_get(black_box(&opool), i);
                }
            });
        });

        let deadpool = shared::setup_deadpool(shared::CAP);
        group.bench_function("deadpool_get", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::deadpool_get(black_box(&deadpool), i);
                }
            });
        });

        let infinity = shared::setup_infinity_pinned(shared::CAP);
        group.bench_function("infinity_pinned", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::infinity_pinned(black_box(&infinity), i);
                }
            });
        });

        let mut infinity_raw = shared::setup_infinity_raw(shared::CAP);
        group.bench_function("infinity_raw", |bencher| {
            bencher.iter(|| {
                for i in 0..N {
                    shared::infinity_raw(black_box(&mut infinity_raw), i);
                }
            });
        });

        group.finish();
    }
}

#[cfg(target_os = "linux")]
criterion::criterion_group!(benches, bench::churn);

#[cfg(target_os = "linux")]
criterion::criterion_main!(benches);
