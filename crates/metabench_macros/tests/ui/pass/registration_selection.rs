// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

extern crate self as metabench;

#[path = "../support.rs"]
mod support;
use metabench_macros::benchmarks;
pub use support::*;

struct FirstBenchmarks;

#[benchmarks]
impl FirstBenchmarks {
    #[metabench_macros::benchmark]
    fn alpha() {}

    #[metabench_macros::benchmark(name = "beta", engines = Engines::ALL)]
    fn renamed() {}

    fn helper() {}
}

struct Other;

#[benchmarks(name = "second")]
impl Other {
    #[metabench_macros::benchmark]
    fn gamma() {}
}

fn register(suite: &mut BenchmarkSuite) {
    <FirstBenchmarks as __private::BenchmarkGroupDefinition>::register(suite);
    <Other as __private::BenchmarkGroupDefinition>::register(suite);
}

fn main() {
    let mut suite = BenchmarkSuite::new(None);
    register(&mut suite);
    assert_eq!(suite.identities(), ["first/alpha", "first/beta", "second/gamma"]);

    let mut selected = BenchmarkSuite::new(Some("first/beta"));
    register(&mut selected);
    assert_eq!(selected.identities(), ["first/beta"]);
}
