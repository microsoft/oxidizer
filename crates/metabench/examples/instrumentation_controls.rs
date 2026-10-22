// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Matched native Callgrind controls for the shared workload boundary.

#![expect(missing_docs, reason = "native Gungraun macros generate undocumented public support items")]

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group, main};

#[metabench::benchmark(TINY, "controls", "tiny")]
#[bench::value(42)]
fn annotated(value: u64) -> u64 {
    black_box(value).wrapping_add(1)
}

#[inline(never)]
fn plain(value: u64) -> u64 {
    black_box(value).wrapping_add(1)
}

#[library_benchmark]
fn annotated_control() -> u64 {
    black_box(annotated(black_box(42)))
}

#[library_benchmark]
fn plain_control() -> u64 {
    black_box(plain(black_box(42)))
}

library_benchmark_group!(
    name = controls;
    benchmarks = annotated_control, plain_control
);
main!(library_benchmark_groups = controls);
