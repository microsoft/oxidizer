// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for affix-edge trie matching at varying sibling counts.
//!
//! Paired with `routerama_affix_match.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "resolved benchmark variants are consumed through black_box")]
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
    use gungraun::prelude::*;

    include!("common/affix_match_scenarios.rs");

    macro_rules! hit_case {
        ($name:ident, $width:literal) => {
            #[library_benchmark]
            #[bench::run(build_affix_hit($width))]
            fn $name(scenario: (RawResolver, String)) -> (RawResolver, String) {
                affix_hit_lookup(&scenario);
                scenario
            }
        };
    }

    hit_case!(hit_02, 2);
    hit_case!(hit_08, 8);
    hit_case!(hit_32, 32);

    macro_rules! miss_case {
        ($name:ident, $width:literal) => {
            #[library_benchmark]
            #[bench::run(build_affix_router($width))]
            fn $name(router: RawResolver) -> RawResolver {
                affix_miss_lookup(&router);
                router
            }
        };
    }

    miss_case!(miss_02, 2);
    miss_case!(miss_08, 8);
    miss_case!(miss_32, 32);

    library_benchmark_group!(
        name = hit;
        benchmarks = hit_02, hit_08, hit_32
    );
    library_benchmark_group!(
        name = miss;
        benchmarks = miss_02, miss_08, miss_32
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{hit, miss};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = hit, miss
);
