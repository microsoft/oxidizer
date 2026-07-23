// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for runtime-mounted services.
//!
//! Paired with `routerama_mount.rs`. The erased-mount cases include their boxed
//! future and `BoxBody` allocations, because those are the cost of opting into
//! the type-erased boundary; the capture, depth, and streaming cases add the
//! documented scratch and error allocations on top of it.

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

    include!("common/mount_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $scenario:expr) => {
            #[library_benchmark]
            #[bench::run(prepare($scenario))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_prepared(prepared))
            }
        };
    }

    benchmark_case!(static_hit_plain_route, Scenario::StaticPlainRoute);
    benchmark_case!(static_hit_populated_erased_mounts, Scenario::StaticWithPopulatedMounts);

    benchmark_case!(dynamic_dispatch_configured_dynamic, Scenario::ConfiguredDynamic);
    benchmark_case!(dynamic_dispatch_erased_mount, Scenario::ErasedMount);

    benchmark_case!(standalone_literal, Scenario::StandaloneLiteral);
    benchmark_case!(standalone_complete_miss, Scenario::StandaloneMiss);

    benchmark_case!(captures_none, Scenario::Captures(CaptureCount::None));
    benchmark_case!(captures_one, Scenario::Captures(CaptureCount::One));
    benchmark_case!(captures_four, Scenario::Captures(CaptureCount::Four));
    benchmark_case!(captures_five, Scenario::Captures(CaptureCount::Five));

    benchmark_case!(streaming_success, Scenario::StreamingSuccess);
    benchmark_case!(streaming_error, Scenario::StreamingError);

    benchmark_case!(depth_segments_16, Scenario::Depth(false));
    benchmark_case!(depth_segments_17, Scenario::Depth(true));

    benchmark_case!(table_size_0016_first, Scenario::Table(TableSize::Mounts16, Position::First));
    benchmark_case!(table_size_0016_middle, Scenario::Table(TableSize::Mounts16, Position::Middle));
    benchmark_case!(table_size_0016_last, Scenario::Table(TableSize::Mounts16, Position::Last));
    benchmark_case!(table_size_0016_miss, Scenario::Table(TableSize::Mounts16, Position::Miss));
    benchmark_case!(table_size_0128_first, Scenario::Table(TableSize::Mounts128, Position::First));
    benchmark_case!(table_size_0128_middle, Scenario::Table(TableSize::Mounts128, Position::Middle));
    benchmark_case!(table_size_0128_last, Scenario::Table(TableSize::Mounts128, Position::Last));
    benchmark_case!(table_size_0128_miss, Scenario::Table(TableSize::Mounts128, Position::Miss));
    benchmark_case!(table_size_1024_first, Scenario::Table(TableSize::Mounts1024, Position::First));
    benchmark_case!(table_size_1024_middle, Scenario::Table(TableSize::Mounts1024, Position::Middle));
    benchmark_case!(table_size_1024_last, Scenario::Table(TableSize::Mounts1024, Position::Last));
    benchmark_case!(table_size_1024_miss, Scenario::Table(TableSize::Mounts1024, Position::Miss));

    library_benchmark_group!(
        name = static_hit;
        benchmarks = static_hit_plain_route, static_hit_populated_erased_mounts
    );
    library_benchmark_group!(
        name = dynamic_dispatch;
        benchmarks = dynamic_dispatch_configured_dynamic, dynamic_dispatch_erased_mount
    );
    library_benchmark_group!(
        name = standalone;
        benchmarks = standalone_literal, standalone_complete_miss
    );
    library_benchmark_group!(
        name = captures;
        benchmarks = captures_none, captures_one, captures_four, captures_five
    );
    library_benchmark_group!(
        name = streaming;
        benchmarks = streaming_success, streaming_error
    );
    library_benchmark_group!(
        name = depth;
        benchmarks = depth_segments_16, depth_segments_17
    );
    library_benchmark_group!(
        name = table_size;
        benchmarks =
            table_size_0016_first,
            table_size_0016_middle,
            table_size_0016_last,
            table_size_0016_miss,
            table_size_0128_first,
            table_size_0128_middle,
            table_size_0128_last,
            table_size_0128_miss,
            table_size_1024_first,
            table_size_1024_middle,
            table_size_1024_last,
            table_size_1024_miss
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{captures, depth, dynamic_dispatch, standalone, static_hit, streaming, table_size};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = static_hit, dynamic_dispatch, standalone, captures, streaming, depth, table_size
);
