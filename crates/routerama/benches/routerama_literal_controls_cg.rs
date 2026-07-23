// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind instruction/cache controls for literal-only route topologies.
//!
//! Paired with `routerama_literal_controls.rs`.

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

    include!("common/literal_control_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $size:ident, $shape:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(RouteSetSize::$size))]
            fn $name(routers: PreparedRouters) -> PreparedRouters {
                std::hint::black_box(run_prepared(&routers, Shape::$shape, Scenario::$scenario));
                routers
            }
        };
    }

    macro_rules! shape_group {
        (
            $group:ident,
            $size:ident,
            $shape:ident,
            $first:ident,
            $middle:ident,
            $last:ident,
            $miss:ident
        ) => {
            benchmark_case!($first, $size, $shape, First);
            benchmark_case!($middle, $size, $shape, Middle);
            benchmark_case!($last, $size, $shape, Last);
            benchmark_case!($miss, $size, $shape, Miss);
            library_benchmark_group!(
                name = $group;
                benchmarks = $first, $middle, $last, $miss
            );
        };
    }

    shape_group!(
        routes_16_wide_fanout,
        Routes16,
        WideFanout,
        routes_16_wide_fanout_first,
        routes_16_wide_fanout_middle,
        routes_16_wide_fanout_last,
        routes_16_wide_fanout_miss
    );
    shape_group!(
        routes_16_deep_chain,
        Routes16,
        DeepChain,
        routes_16_deep_chain_first,
        routes_16_deep_chain_middle,
        routes_16_deep_chain_last,
        routes_16_deep_chain_miss
    );
    shape_group!(
        routes_16_terminal_suffix_shared_prefix,
        Routes16,
        TerminalSuffix,
        routes_16_terminal_suffix_shared_prefix_first,
        routes_16_terminal_suffix_shared_prefix_middle,
        routes_16_terminal_suffix_shared_prefix_last,
        routes_16_terminal_suffix_shared_prefix_miss
    );
    shape_group!(
        routes_128_wide_fanout,
        Routes128,
        WideFanout,
        routes_128_wide_fanout_first,
        routes_128_wide_fanout_middle,
        routes_128_wide_fanout_last,
        routes_128_wide_fanout_miss
    );
    shape_group!(
        routes_128_deep_chain,
        Routes128,
        DeepChain,
        routes_128_deep_chain_first,
        routes_128_deep_chain_middle,
        routes_128_deep_chain_last,
        routes_128_deep_chain_miss
    );
    shape_group!(
        routes_128_terminal_suffix_shared_prefix,
        Routes128,
        TerminalSuffix,
        routes_128_terminal_suffix_shared_prefix_first,
        routes_128_terminal_suffix_shared_prefix_middle,
        routes_128_terminal_suffix_shared_prefix_last,
        routes_128_terminal_suffix_shared_prefix_miss
    );
    shape_group!(
        routes_1024_wide_fanout,
        Routes1024,
        WideFanout,
        routes_1024_wide_fanout_first,
        routes_1024_wide_fanout_middle,
        routes_1024_wide_fanout_last,
        routes_1024_wide_fanout_miss
    );
    shape_group!(
        routes_1024_deep_chain,
        Routes1024,
        DeepChain,
        routes_1024_deep_chain_first,
        routes_1024_deep_chain_middle,
        routes_1024_deep_chain_last,
        routes_1024_deep_chain_miss
    );
    shape_group!(
        routes_1024_terminal_suffix_shared_prefix,
        Routes1024,
        TerminalSuffix,
        routes_1024_terminal_suffix_shared_prefix_first,
        routes_1024_terminal_suffix_shared_prefix_middle,
        routes_1024_terminal_suffix_shared_prefix_last,
        routes_1024_terminal_suffix_shared_prefix_miss
    );
}

#[cfg(target_os = "linux")]
pub use linux::{
    routes_16_deep_chain, routes_16_terminal_suffix_shared_prefix, routes_16_wide_fanout, routes_128_deep_chain,
    routes_128_terminal_suffix_shared_prefix, routes_128_wide_fanout, routes_1024_deep_chain, routes_1024_terminal_suffix_shared_prefix,
    routes_1024_wide_fanout,
};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = gungraun::LibraryBenchmarkConfig::default().tool(
        gungraun::Callgrind::with_args(["--branch-sim=yes", "--cache-sim=yes"])
    );
    library_benchmark_groups =
        routes_16_wide_fanout,
        routes_16_deep_chain,
        routes_16_terminal_suffix_shared_prefix,
        routes_128_wide_fanout,
        routes_128_deep_chain,
        routes_128_terminal_suffix_shared_prefix,
        routes_1024_wide_fanout,
        routes_1024_deep_chain,
        routes_1024_terminal_suffix_shared_prefix
);
