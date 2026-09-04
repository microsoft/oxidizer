// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stateful benchmarks without data-driven cases.

use metabench::SimpleFixture;

pub(crate) struct VecBenchmarks {
    values: Vec<u64>,
}

impl SimpleFixture for VecBenchmarks {
    fn setup() -> Self {
        Self {
            values: (0..1_000).rev().collect(),
        }
    }
}

#[metabench::benchmarks]
impl VecBenchmarks {
    #[metabench::benchmark]
    fn sort(&mut self) {
        self.values.sort_unstable();
    }

    #[metabench::benchmark]
    fn contains(&self) -> bool {
        self.values.contains(&500)
    }
}

metabench::main!(groups = [VecBenchmarks], allocator = std::alloc::System,);
