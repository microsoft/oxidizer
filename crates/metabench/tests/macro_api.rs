// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Gungraun macros generate undocumented support items")]
#![expect(
    clippy::unnecessary_wraps,
    unreachable_pub,
    reason = "Gungraun macros generate adapter and configuration functions"
)]

use gungraun::LibraryBenchmarkConfig;

const ARITHMETIC_GROUP: &str = "arithmetic";

#[metabench::benchmark(
    identity = INCREMENT,
    group_name = ARITHMETIC_GROUP,
    benchmark_name = "increment",
    gungraun_config = LibraryBenchmarkConfig::default()
)]
#[bench::small(1_u64)]
/// Increments the supplied value.
fn increment_benchmark(value: u64) -> u64 {
    value + 1
}

#[test]
fn benchmark_function_remains_callable() {
    assert_eq!(increment_benchmark(41), 42);
    assert_eq!(INCREMENT.group_name(), "arithmetic");
    assert_eq!(INCREMENT.benchmark_name(), "increment");
}
