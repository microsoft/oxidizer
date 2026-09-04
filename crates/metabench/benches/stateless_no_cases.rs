// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stateless benchmarks without data-driven cases.

pub(crate) struct MathBenchmarks;

#[metabench::benchmarks]
impl MathBenchmarks {
    #[metabench::benchmark]
    fn fibonacci_10k() -> u64 {
        (0..10_000_u64).fold(0, |accumulator, value| accumulator ^ value)
    }
}

metabench::main!(groups = [MathBenchmarks], allocator = std::alloc::System,);
