// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `GlobalPool` reservation paths in the `bytesbuf` package.
//!
//! Paired with `global_pool.rs`, which covers the same operations under wall-clock measurement.
//! `alloc_tiny` exercises the single-block reserve fast path and `alloc_1mb` the multi-block path;
//! both pair with the Criterion `GlobalPool/alloc_tiny` and `GlobalPool/alloc_1mb` benchmarks (the
//! Callgrind variants isolate a single reservation).
//!
//! Each setup function pre-warms the relevant sub-pool outside the measured region so the timed
//! reservation reuses pooled blocks rather than the system allocator, and the benchmark returns
//! both the pool and the reserved buffer so their drops are not counted.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use bytesbuf::BytesBuf;
    use bytesbuf::mem::GlobalPool;
    use gungraun::{library_benchmark, library_benchmark_group};

    const ONE_MB: usize = 1024 * 1024;
    const TINY: usize = 128;

    // Reaches a steady state where the relevant sub-pool already holds enough free blocks to serve
    // the measured reservation, so the timed body never touches the system allocator.
    const WARMUP_CYCLES: usize = 4;

    fn warm_pool(reserve_bytes: usize) -> GlobalPool {
        let memory = GlobalPool::new();

        for _ in 0..WARMUP_CYCLES {
            drop(memory.reserve(reserve_bytes));
        }

        memory
    }

    // Single-block reserve: a tiny request fits in one block from the smallest sub-pool.
    #[library_benchmark]
    #[bench::single_block(warm_pool(TINY))]
    fn global_pool_alloc_tiny(memory: GlobalPool) -> (GlobalPool, BytesBuf) {
        let buf = memory.reserve(black_box(TINY));
        (memory, buf)
    }

    // Multi-block reserve: a large request spans several blocks of the largest sub-pool.
    #[library_benchmark]
    #[bench::multi_block(warm_pool(ONE_MB))]
    fn global_pool_alloc_1mb(memory: GlobalPool) -> (GlobalPool, BytesBuf) {
        let buf = memory.reserve(black_box(ONE_MB));
        (memory, buf)
    }

    library_benchmark_group!(
        name = global_pool;
        benchmarks = global_pool_alloc_tiny, global_pool_alloc_1mb
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::global_pool;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = global_pool
);
