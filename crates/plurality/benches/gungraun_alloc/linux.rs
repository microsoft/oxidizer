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

//! Callgrind (instruction-count) benchmarks for every allocation function, run
//! via [gungraun]. Each benchmark runs the hot operation **once** — Callgrind
//! counts instructions exactly, so no loop is needed.
//!
//! This mirrors `benches/criterion_alloc.rs` 1:1: every benchmark here calls the
//! same `ops::<name>` body that the criterion suite loops `N` times. The
//! `perf_report.rs` script aligns the two by `<group>/<name>`.
//!
//! Run with: `cargo bench --bench gungraun_alloc` (needs `valgrind`).
//!
//! [gungraun]: https://github.com/gungraun/gungraun

use std::hint::black_box;

use gungraun::prelude::*;
use plurality::{Arc, Pool, Rc};

#[path = "../shared/ops.rs"]
mod ops;

use ops::Obj;

/// Defines a `#[library_benchmark]` that runs `ops::<name>` once against a
/// pre-warmed pool (returned from the timed region so teardown isn't measured).
macro_rules! alloc_bench {
    ($name:ident) => {
        #[library_benchmark]
        #[bench::op(args = (ops::CAP,), setup = ops::setup_pool)]
        fn $name(pool: Pool<Obj>) -> Pool<Obj> {
            ops::$name(black_box(&pool), 0);
            pool
        }
    };
}

alloc_bench!(box_val);
alloc_bench!(box_with);
alloc_bench!(box_uninit);
alloc_bench!(arc_val);
alloc_bench!(arc_with);
alloc_bench!(arc_uninit);
alloc_bench!(alloc_val);
alloc_bench!(alloc_with);
alloc_bench!(alloc_uninit);
alloc_bench!(rc_val);
alloc_bench!(rc_with);
alloc_bench!(rc_uninit);

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_arc)]
fn arc_clone((pool, base): (Pool<Obj>, Arc<Obj>)) -> (Pool<Obj>, Arc<Obj>) {
    ops::arc_clone(black_box(&base));
    (pool, base)
}

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_rc)]
fn rc_clone((pool, base): (Pool<Obj>, Rc<Obj>)) -> (Pool<Obj>, Rc<Obj>) {
    ops::rc_clone(black_box(&base));
    (pool, base)
}

library_benchmark_group!(
    name = alloc,
    benchmarks = [
        box_val,
        box_with,
        box_uninit,
        arc_val,
        arc_with,
        arc_uninit,
        alloc_val,
        alloc_with,
        alloc_uninit,
        rc_val,
        rc_with,
        rc_uninit
    ]
);

library_benchmark_group!(name = clone, benchmarks = [arc_clone, rc_clone]);
