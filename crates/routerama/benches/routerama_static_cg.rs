// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Focused Callgrind benchmarks for generated static resolver branches.
//!
//! Paired with `routerama_static.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared scenario module supports both benchmark harnesses")]
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

    include!("common/static_scenarios.rs");

    macro_rules! bench_case {
        ($name:ident, $resolver:ty, $setup:ident, $run:ident) => {
            #[library_benchmark]
            #[bench::run($setup())]
            fn $name(router: $resolver) -> $resolver {
                $run(&router);
                router
            }
        };
    }

    bench_case!(
        hits_shallow_literal,
        StaticScenarioRouter,
        build_static_scenario,
        static_shallow_literal
    );
    bench_case!(hits_deep_literal, StaticScenarioRouter, build_static_scenario, static_deep_literal);
    bench_case!(hits_fanout_first, StaticScenarioRouter, build_static_scenario, static_fanout_first);
    bench_case!(
        hits_fanout_middle,
        StaticScenarioRouter,
        build_static_scenario,
        static_fanout_middle
    );
    bench_case!(hits_fanout_last, StaticScenarioRouter, build_static_scenario, static_fanout_last);
    bench_case!(hits_borrow_one, StaticScenarioRouter, build_static_scenario, static_borrow_one);
    bench_case!(hits_borrow_four, StaticScenarioRouter, build_static_scenario, static_borrow_four);
    bench_case!(hits_parse_number, StaticScenarioRouter, build_static_scenario, static_parse_number);
    bench_case!(hits_own_plain, StaticScenarioRouter, build_static_scenario, static_own_plain);
    bench_case!(hits_own_percent, StaticScenarioRouter, build_static_scenario, static_own_percent);

    bench_case!(misses_early, StaticScenarioRouter, build_static_scenario, static_early_miss);
    bench_case!(misses_late, StaticScenarioRouter, build_static_scenario, static_late_miss);
    bench_case!(
        misses_pathological_long,
        StaticScenarioRouter,
        build_static_scenario,
        static_pathological_long_miss
    );
    bench_case!(
        misses_wrong_method,
        StaticScenarioRouter,
        build_static_scenario,
        static_wrong_method
    );

    bench_case!(features_rest, StaticScenarioRouter, build_static_scenario, static_rest);
    bench_case!(features_affix, StaticScenarioRouter, build_static_scenario, static_affix);
    bench_case!(features_no_verb_table, NoVerbScenarioRouter, build_no_verb_scenario, static_no_verb);
    bench_case!(
        features_verb_table_nonverb_hit,
        WithVerbScenarioRouter,
        build_with_verb_scenario,
        static_with_verb_nonverb_hit
    );
    bench_case!(
        features_verb_hit,
        WithVerbScenarioRouter,
        build_with_verb_scenario,
        static_with_verb_hit
    );

    bench_case!(
        table_shape_shallow_table_hit,
        ShallowTableRouter,
        build_shallow_table,
        static_shallow_table_hit
    );
    bench_case!(
        table_shape_deep_outlier_table_hit,
        DeepOutlierTableRouter,
        build_deep_outlier_table,
        static_deep_outlier_table_hit
    );
    bench_case!(
        table_shape_affix_fanout_first,
        AffixFanoutRouter,
        build_affix_fanout,
        static_affix_fanout_first
    );
    bench_case!(
        table_shape_affix_fanout_middle,
        AffixFanoutRouter,
        build_affix_fanout,
        static_affix_fanout_middle
    );
    bench_case!(
        table_shape_affix_fanout_last,
        AffixFanoutRouter,
        build_affix_fanout,
        static_affix_fanout_last
    );

    library_benchmark_group!(
        name = hits;
        benchmarks =
            hits_shallow_literal,
            hits_deep_literal,
            hits_fanout_first,
            hits_fanout_middle,
            hits_fanout_last,
            hits_borrow_one,
            hits_borrow_four,
            hits_parse_number,
            hits_own_plain,
            hits_own_percent
    );
    library_benchmark_group!(
        name = misses;
        benchmarks = misses_early, misses_late, misses_pathological_long, misses_wrong_method
    );
    library_benchmark_group!(
        name = features;
        benchmarks =
            features_rest,
            features_affix,
            features_no_verb_table,
            features_verb_table_nonverb_hit,
            features_verb_hit
    );
    library_benchmark_group!(
        name = table_shape;
        benchmarks =
            table_shape_shallow_table_hit,
            table_shape_deep_outlier_table_hit,
            table_shape_affix_fanout_first,
            table_shape_affix_fanout_middle,
            table_shape_affix_fanout_last
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{features, hits, misses, table_shape};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = hits, misses, features, table_shape
);
