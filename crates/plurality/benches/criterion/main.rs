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
//! `N` operations; `perf_report.rs` converts them to per-operation times.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

mod ops;

use ops::Obj;

/// Operations performed per criterion iteration. Mirrors the "run once" of the
/// gungraun suite, scaled up so wall-clock timing has signal.
const N: u64 = 1000;

fn alloc_benches(c: &mut Criterion) {
    let pool = ops::setup_pool(ops::CAP);

    let mut g = c.benchmark_group("alloc");
    macro_rules! bench {
        ($name:ident) => {
            g.bench_function(stringify!($name), |b| {
                b.iter(|| {
                    for i in 0..N {
                        ops::$name(black_box(&pool), i);
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
    g.finish();

    let mut g = c.benchmark_group("clone");
    let arc_base = pool.alloc_arc(Obj::new(0));
    g.bench_function("arc_clone", |b| {
        b.iter(|| {
            for _ in 0..N {
                ops::arc_clone(black_box(&arc_base));
            }
        });
    });
    let rc_base = pool.alloc_rc(Obj::new(0));
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
