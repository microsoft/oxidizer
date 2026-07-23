// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for Routerama route policy.
//!
//! Paired with `routerama_route_policy.rs`.

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
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
#[expect(
    clippy::panic,
    reason = "a pending or malformed in-memory fixture is a benchmark invariant violation"
)]
mod linux {
    use gungraun::prelude::*;

    include!("common/route_policy_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_prepared(prepared))
            }
        };
    }

    benchmark_case!(priority_plain_route, PriorityPlain);
    benchmark_case!(priority_highest_candidate, PriorityHighestCandidate);
    benchmark_case!(priority_lower_candidate, PriorityLowerCandidate);

    benchmark_case!(predicates_unconstrained, PredicateUnconstrained);
    benchmark_case!(predicates_accepted, PredicateAccepted);
    benchmark_case!(predicates_unsupported_media_type, PredicateUnsupportedMediaType);
    benchmark_case!(predicates_not_acceptable, PredicateNotAcceptable);

    benchmark_case!(fallback_default_miss, FallbackDefaultMiss);
    benchmark_case!(fallback_typed_miss, FallbackTypedMiss);

    benchmark_case!(catcher_default_rejection, CatcherDefaultRejection);
    benchmark_case!(catcher_typed_rejection, CatcherTypedRejection);

    library_benchmark_group!(
        name = priority;
        benchmarks = priority_plain_route, priority_highest_candidate, priority_lower_candidate
    );
    library_benchmark_group!(
        name = predicates;
        benchmarks =
            predicates_unconstrained,
            predicates_accepted,
            predicates_unsupported_media_type,
            predicates_not_acceptable
    );
    library_benchmark_group!(
        name = fallback;
        benchmarks = fallback_default_miss, fallback_typed_miss
    );
    library_benchmark_group!(
        name = catcher;
        benchmarks = catcher_default_rejection, catcher_typed_rejection
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{catcher, fallback, predicates, priority};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = priority, predicates, fallback, catcher
);
