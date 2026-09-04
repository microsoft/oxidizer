// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stateless benchmarks with data-driven cases.

use metabench::{BenchmarkCase, BenchmarkCases};

#[derive(Clone, Copy)]
pub(crate) struct SearchCase {
    name: &'static str,
    haystack: &'static str,
    needle: char,
}

impl BenchmarkCase for SearchCase {
    fn name(&self) -> String {
        self.name.to_owned()
    }
}

pub(crate) struct SearchBenchmarks;

impl BenchmarkCases for SearchBenchmarks {
    type Case = SearchCase;

    fn cases() -> impl IntoIterator<Item = Self::Case> {
        [
            SearchCase {
                name: "short",
                haystack: "metabench",
                needle: 'b',
            },
            SearchCase {
                name: "long",
                haystack: "a fast unified benchmark harness for Rust",
                needle: 'R',
            },
        ]
    }
}

#[metabench::benchmarks]
impl SearchBenchmarks {
    #[metabench::benchmark]
    fn find_character(case: &SearchCase) -> Option<usize> {
        case.haystack.find(case.needle)
    }
}

metabench::main!(groups = [SearchBenchmarks], allocator = std::alloc::System,);
