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

//! Wall-clock (criterion) benchmarks for every allocation function.
//!
//! This mirrors `benches/gungraun_alloc.rs` 1:1: every benchmark loops the same
//! `ops::<name>` body that the gungraun suite runs once. Each criterion
//! iteration performs `N` operations, so the reported time is for `N` ops
//! (divide by `N` for per-operation cost — `perf_report.rs` does this).
//!
//! Run with: `cargo bench --bench criterion_alloc`.

#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(missing_debug_implementations, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "shared/ops.rs"]
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

criterion_group!(benches, alloc_benches);
criterion_main!(benches);
