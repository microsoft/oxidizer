// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, non_snake_case)]

use renamed_metabench::__private::gungraun::LibraryBenchmarkConfig;
use renamed_metabench::{BenchmarkIdentity, benchmark};

const GROUP: &str = "group";

fn config() -> LibraryBenchmarkConfig {
    LibraryBenchmarkConfig::default()
}

fn setup() -> u64 {
    2
}
fn teardown(_: u64) {}

#[benchmark(POSITIONAL, GROUP, "positional")]
fn positional() -> u64 {
    1
}

#[benchmark(
    identity = NAMED,
    group_name = GROUP,
    benchmark_name = "named",
    gungraun_config = config(),
    gungraun_setup = setup,
    gungraun_teardown = teardown
)]
fn named(value: u64) -> u64 {
    value
}

#[benchmark(CONDITIONAL_BENCH, GROUP, "conditional-bench")]
#[cfg_attr(all(), bench::enabled(3_u64))]
#[cfg_attr(any(), bench::disabled(4_u64))]
fn conditional_bench(value: u64) -> u64 {
    value
}

#[benchmark(CONDITIONAL_BENCHES, GROUP, "conditional-benches")]
#[cfg_attr(all(), benches::enabled(iter = 1_u64..=2))]
#[cfg_attr(any(), benches::disabled(iter = 3_u64..=4))]
fn conditional_benches(value: u64) -> u64 {
    value
}

#[benchmark(GENERIC, GROUP, "generic")]
#[bench::u64_value(5_u64)]
fn generic<T: Copy>(value: T) -> T {
    value
}

#[benchmark(CONST_FN, GROUP, "const")]
const fn const_fn() -> u64 {
    6
}

#[benchmark(SAFE_WRAPPER, GROUP, "safe-wrapper")]
fn safe_wrapper() -> u64 {
    // SAFETY: `&value` is a valid, aligned reference for the duration of the read.
    // This demonstrates the safe-wrapper pattern recommended for benchmarking logic
    // that needs `unsafe`, since `#[benchmark]` itself rejects `unsafe fn` items.
    let value = 7_u64;
    unsafe { core::ptr::read(&value) }
}

#[benchmark(ABI_FN, GROUP, "abi")]
extern "C" fn abi_fn() -> u64 {
    8
}

fn assert_identity(_: BenchmarkIdentity) {}

fn main() {
    assert_identity(POSITIONAL);
    assert_identity(NAMED);
    assert_identity(CONDITIONAL_BENCH);
    assert_identity(CONDITIONAL_BENCHES);
    assert_identity(GENERIC);
    assert_identity(CONST_FN);
    assert_identity(SAFE_WRAPPER);
    assert_identity(ABI_FN);
}
