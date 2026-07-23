// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for mixed static and runtime routing.
//!
//! Paired with `routerama_mixed.rs`.

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
fn main() {}

#[cfg(target_os = "linux")]
#[expect(
    clippy::unnecessary_box_returns,
    reason = "returning boxed setup state excludes large resolver moves and teardown from measured paths"
)]
mod linux {
    use gungraun::prelude::*;

    include!("common/mixed_scenarios.rs");

    macro_rules! mixed_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(build_mixed_scenario())]
            fn $name(router: Box<MixedScenarioResolver>) -> Box<MixedScenarioResolver> {
                std::hint::black_box(run_scenario(&router, Scenario::$scenario));
                router
            }
        };
    }

    mixed_case!(dispatch_short_static_hit, ShortStaticHit);
    mixed_case!(dispatch_short_dynamic_hit, ShortDynamicHit);
    mixed_case!(dispatch_short_miss, ShortMiss);
    mixed_case!(dispatch_segments_17_static_hit, Deep17StaticHit);
    mixed_case!(dispatch_segments_17_dynamic_hit, Deep17DynamicHit);
    mixed_case!(dispatch_segments_17_miss, Deep17Miss);
    mixed_case!(dispatch_segments_32_static_hit, Deep32StaticHit);
    mixed_case!(dispatch_segments_32_dynamic_hit, Deep32DynamicHit);
    mixed_case!(dispatch_segments_32_miss, Deep32Miss);

    library_benchmark_group!(
        name = dispatch;
        benchmarks =
            dispatch_short_static_hit,
            dispatch_short_dynamic_hit,
            dispatch_short_miss,
            dispatch_segments_17_static_hit,
            dispatch_segments_17_dynamic_hit,
            dispatch_segments_17_miss,
            dispatch_segments_32_static_hit,
            dispatch_segments_32_dynamic_hit,
            dispatch_segments_32_miss
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::dispatch;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes", "--cache-sim=yes"]));
    library_benchmark_groups = dispatch
);
