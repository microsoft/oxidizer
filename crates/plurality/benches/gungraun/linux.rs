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
    clippy::used_underscore_binding,
    reason = "test and benchmark code"
)]
#![allow(missing_docs, reason = "benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]

//! Callgrind (instruction-count) benchmarks, run via [gungraun]. Each benchmark
//! runs the hot operation **once** — Callgrind counts instructions exactly, so
//! no loop is needed. Every body here is also looped by the criterion suite; the
//! `perf_report.rs` script aligns the two by `<group>/<name>`.
//!
//! [gungraun]: https://github.com/gungraun/gungraun

use std::hint::black_box;

use gungraun::prelude::*;
use infinity_pool::{BlindPool, LocalBlindPool, LocalPinnedPool, PinnedPool};
use plurality::{Arc, Pool, Rc};

use crate::ops::{self, Obj};

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
alloc_bench!(box_unsize);
alloc_bench!(arc_unsize);
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

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_plurality)]
fn plurality_box(pool: Pool<Obj>) -> Pool<Obj> {
    ops::plurality_box(black_box(&pool), 0);
    pool
}

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_infinity_pinned)]
fn infinity_pinned(pool: PinnedPool<Obj>) -> PinnedPool<Obj> {
    ops::infinity_pinned(black_box(&pool), 0);
    pool
}

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_infinity_local_pinned)]
fn infinity_local_pinned(pool: LocalPinnedPool<Obj>) -> LocalPinnedPool<Obj> {
    ops::infinity_local_pinned(black_box(&pool), 0);
    pool
}

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_infinity_blind)]
fn infinity_blind(pool: BlindPool) -> BlindPool {
    ops::infinity_blind(black_box(&pool), 0);
    pool
}

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_infinity_local_blind)]
fn infinity_local_blind(pool: LocalBlindPool) -> LocalBlindPool {
    ops::infinity_local_blind(black_box(&pool), 0);
    pool
}

#[library_benchmark]
#[bench::op(args = (ops::CAP,), setup = ops::setup_std_box)]
fn std_box(_setup: ()) {
    ops::std_box(0);
}

library_benchmark_group!(
    name = alloc,
    benchmarks = [
        box_val,
        box_with,
        box_uninit,
        box_unsize,
        arc_unsize,
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

library_benchmark_group!(
    name = dyn_box,
    benchmarks = [
        plurality_box,
        infinity_pinned,
        infinity_local_pinned,
        infinity_blind,
        infinity_local_blind,
        std_box
    ]
);
