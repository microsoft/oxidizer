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
mod linux {
    use gungraun::prelude::*;

    include!("common/mixed_scenarios.rs");

    macro_rules! mixed_case {
        ($name:ident, $run:ident) => {
            #[library_benchmark]
            #[bench::run(build_mixed_scenario())]
            fn $name(router: MixedScenarioResolver) -> MixedScenarioResolver {
                $run(&router);
                router
            }
        };
    }

    mixed_case!(dispatch_static_hit, mixed_static_hit);
    mixed_case!(dispatch_dynamic_fallback_hit, mixed_dynamic_hit);
    mixed_case!(dispatch_complete_miss, mixed_complete_miss);
    mixed_case!(dispatch_static_capture_error, mixed_static_capture_error);

    library_benchmark_group!(
        name = dispatch;
        benchmarks =
            dispatch_static_hit,
            dispatch_dynamic_fallback_hit,
            dispatch_complete_miss,
            dispatch_static_capture_error
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
