// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind baselines for generated route-predicate overlap groups.
//!
//! Paired with `routerama_predicate_overlap.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::prelude::*;

    include!("common/predicate_overlap_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $size:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(GroupSize::$size, Scenario::$scenario))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_prepared(prepared))
            }
        };
    }

    macro_rules! overlap_group {
        (
            $group:ident,
            $size:ident,
            $first:ident,
            $middle:ident,
            $last:ident,
            $miss:ident,
            $malformed:ident,
            $multiple_accept:ident,
            $multiple_content_type:ident,
            $multiple_host:ident
        ) => {
            benchmark_case!($first, $size, First);
            benchmark_case!($middle, $size, Middle);
            benchmark_case!($last, $size, Last);
            benchmark_case!($miss, $size, Miss);
            benchmark_case!($malformed, $size, MalformedAccept);
            benchmark_case!($multiple_accept, $size, MultipleAccept);
            benchmark_case!($multiple_content_type, $size, MultipleContentType);
            benchmark_case!($multiple_host, $size, MultipleHost);
            library_benchmark_group!(
                name = $group;
                benchmarks =
                    $first,
                    $middle,
                    $last,
                    $miss,
                    $malformed,
                    $multiple_accept,
                    $multiple_content_type,
                    $multiple_host
            );
        };
    }

    overlap_group!(
        overlap_2,
        Two,
        overlap_2_winner_first,
        overlap_2_winner_middle,
        overlap_2_winner_last,
        overlap_2_miss,
        overlap_2_malformed_accept,
        overlap_2_multiple_accept,
        overlap_2_multiple_content_type,
        overlap_2_multiple_host
    );
    overlap_group!(
        overlap_8,
        Eight,
        overlap_8_winner_first,
        overlap_8_winner_middle,
        overlap_8_winner_last,
        overlap_8_miss,
        overlap_8_malformed_accept,
        overlap_8_multiple_accept,
        overlap_8_multiple_content_type,
        overlap_8_multiple_host
    );
    overlap_group!(
        overlap_32,
        ThirtyTwo,
        overlap_32_winner_first,
        overlap_32_winner_middle,
        overlap_32_winner_last,
        overlap_32_miss,
        overlap_32_malformed_accept,
        overlap_32_multiple_accept,
        overlap_32_multiple_content_type,
        overlap_32_multiple_host
    );
}

#[cfg(target_os = "linux")]
pub use linux::{overlap_2, overlap_8, overlap_32};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = overlap_2, overlap_8, overlap_32);
