// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

extern crate self as metabench;

#[path = "../support.rs"]
mod support;
use metabench_macros::benchmarks;
pub use support::*;

struct Size(&'static str);

impl BenchmarkCase for Size {
    fn name(&self) -> String {
        self.0.to_owned()
    }
}

struct Stateless;

#[benchmarks]
impl Stateless {
    #[metabench_macros::benchmark(name = "renamed", engines = Engines::CRITERION)]
    fn work() -> usize {
        1
    }
}

struct StatelessCases;

impl BenchmarkCases for StatelessCases {
    type Case = Size;

    fn cases() -> impl IntoIterator<Item = Self::Case> {
        [Size("small"), Size("large")]
    }
}

#[benchmarks]
impl StatelessCases {
    #[metabench_macros::benchmark]
    fn work(case: &Size) -> usize {
        case.0.len()
    }
}

struct Stateful(usize);

impl SimpleFixture for Stateful {
    fn setup() -> Self {
        Self(0)
    }
}

#[benchmarks]
impl Stateful {
    #[metabench_macros::benchmark]
    fn read(&self) -> usize {
        self.0
    }

    #[metabench_macros::benchmark]
    fn write(&mut self) {
        self.0 += 1;
    }
}

struct StatefulCases(usize);

impl Fixture for StatefulCases {
    type Case = Size;

    fn cases() -> impl IntoIterator<Item = Self::Case> {
        [Size("small")]
    }

    fn setup(case: &Self::Case) -> Self {
        Self(case.0.len())
    }
}

#[benchmarks(name = "stateful_data")]
impl StatefulCases {
    #[metabench_macros::benchmark]
    fn consume(&mut self) -> usize {
        self.0
    }
}

fn main() {}
