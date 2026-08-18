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
    clippy::missing_panics_doc,
    missing_debug_implementations,
    missing_docs,
    reason = "benchmark code"
)]

//! Criterion allocation and fat-pointer benchmarks. Reported iterations contain
//! `N` operations; `perf_report.rs` converts them to per-operation times. A row
//! whose measurement requires a first-touch state runs one operation per
//! iteration and says so at its definition.

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

// The shared module is compiled into each bench target, matching the multitude
// benchmark pattern while keeping the optimizer's same-crate view of the hot
// path. Ref: docs/callgrind-benchmarks.md, "Pairing with Criterion".
#[path = "../plurality_ops_common/mod.rs"]
mod ops;

/// Operations performed per criterion iteration. Mirrors the "run once" of the
/// gungraun suite, scaled up so wall-clock timing has signal.
const N: u64 = 1000;

fn alloc_benches(c: &mut Criterion) {
    let pool = ops::setup_pool(ops::CAP);
    let multi = ops::setup_multi_pool(ops::CAP);
    let multi_spread = ops::setup_multi_pool_spread(ops::CAP);

    let mut g = c.benchmark_group("alloc");
    macro_rules! bench {
        ($name:ident) => {
            bench!($name, &pool);
        };
        ($name:ident, $pool:expr) => {
            g.bench_function(stringify!($name), |b| {
                b.iter(|| {
                    for i in 0..N {
                        ops::$name(black_box($pool), i);
                    }
                });
            });
        };
    }
    bench!(box_val);
    bench!(box_with);
    bench!(box_uninit);
    bench!(box_unsize);
    bench!(arc_unsize);
    bench!(arc_val);
    bench!(arc_with);
    bench!(arc_uninit);
    bench!(alloc_val);
    bench!(alloc_with);
    bench!(alloc_uninit);
    bench!(rc_val);
    bench!(rc_with);
    bench!(rc_uninit);
    bench!(multi_box_val, &multi);
    bench!(multi_box_val_spread, &multi_spread);

    // A miss is available only on a pool that has not yet seen the layout, so
    // every measured iteration takes its own pool. `iter_batched` builds the
    // batch before the timed region and drops the returned pools after it, so
    // neither construction nor teardown is measured, and this row therefore
    // reports one operation per iteration rather than `N`. The batch holds one
    // pool per iteration, each carrying the chunk its miss installed, so it is
    // sized for large inputs to bound the memory a batch occupies.
    //
    // The row prices the whole miss — the failed scan, the layout pool, and its
    // first chunk — since that is what a first touch costs; it does not isolate
    // the scan.
    g.bench_function("multi_box_val_miss", |b| {
        b.iter_batched(
            || ops::setup_multi_pool_miss(ops::CAP),
            |pool| {
                ops::multi_box_val_miss(black_box(&pool), 0);
                pool
            },
            BatchSize::LargeInput,
        );
    });
    g.finish();

    let mut g = c.benchmark_group("clone");
    let (_arc_pool, arc_base) = ops::setup_arc(ops::CAP);
    g.bench_function("arc_clone", |b| {
        b.iter(|| {
            for _ in 0..N {
                ops::arc_clone(black_box(&arc_base));
            }
        });
    });
    let (_rc_pool, rc_base) = ops::setup_rc(ops::CAP);
    g.bench_function("rc_clone", |b| {
        b.iter(|| {
            for _ in 0..N {
                ops::rc_clone(black_box(&rc_base));
            }
        });
    });
    g.finish();
}

fn dyn_box_benches(c: &mut Criterion) {
    let plurality = ops::setup_plurality(ops::CAP);
    let plurality_multi = ops::setup_plurality_multi(ops::CAP);
    let infinity = ops::setup_infinity_pinned(ops::CAP);
    let infinity_local = ops::setup_infinity_local_pinned(ops::CAP);
    let infinity_blind = ops::setup_infinity_blind(ops::CAP);
    let infinity_local_blind = ops::setup_infinity_local_blind(ops::CAP);
    ops::setup_std_box(ops::CAP);

    let mut group = c.benchmark_group("dyn_box");
    group.bench_function("plurality_box", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::plurality_box(black_box(&plurality), i);
            }
        });
    });
    group.bench_function("plurality_multi_box", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::plurality_multi_box(black_box(&plurality_multi), i);
            }
        });
    });
    group.bench_function("infinity_pinned", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::infinity_pinned(black_box(&infinity), i);
            }
        });
    });
    group.bench_function("infinity_local_pinned", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::infinity_local_pinned(black_box(&infinity_local), i);
            }
        });
    });
    group.bench_function("infinity_blind", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::infinity_blind(black_box(&infinity_blind), i);
            }
        });
    });
    group.bench_function("infinity_local_blind", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::infinity_local_blind(black_box(&infinity_local_blind), i);
            }
        });
    });
    group.bench_function("std_box", |b| {
        b.iter(|| {
            for i in 0..N {
                ops::std_box(i);
            }
        });
    });
    group.finish();
}

criterion_group!(benches, alloc_benches, dyn_box_benches);
criterion_main!(benches);
