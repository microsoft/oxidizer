// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Focused Callgrind benchmarks for runtime-only router behavior.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "resolved benchmark variants are consumed through black_box")]
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

    include!("common/dynamic_scenarios.rs");

    macro_rules! typed_case {
        ($name:ident, $run:ident) => {
            #[library_benchmark]
            #[bench::run(build_dynamic_typed())]
            fn $name(router: DynamicTypedScenarioResolver) -> DynamicTypedScenarioResolver {
                $run(&router);
                router
            }
        };
    }

    typed_case!(typed_unit, dynamic_typed_unit);
    typed_case!(typed_parse, dynamic_typed_parse);
    typed_case!(typed_owned_plain, dynamic_typed_owned_plain);
    typed_case!(typed_owned_percent, dynamic_typed_owned_percent);

    macro_rules! fanout_case {
        ($name:ident, $width:literal) => {
            #[library_benchmark]
            #[bench::run(build_dynamic_fanout($width))]
            fn $name(scenario: (RawResolver, String)) -> (RawResolver, String) {
                dynamic_fanout_lookup(&scenario);
                scenario
            }
        };
    }

    fanout_case!(fanout_01, 1);
    fanout_case!(fanout_02, 2);
    fanout_case!(fanout_04, 4);
    fanout_case!(fanout_08, 8);
    fanout_case!(fanout_16, 16);
    fanout_case!(fanout_32, 32);
    fanout_case!(fanout_64, 64);

    macro_rules! tuple_case {
        ($name:ident, $setup:expr, $run:ident) => {
            #[library_benchmark]
            #[bench::run($setup)]
            fn $name(scenario: (RawResolver, String)) -> (RawResolver, String) {
                $run(&scenario);
                scenario
            }
        };
    }

    tuple_case!(capture_count_4, build_capture_threshold(4), dynamic_capture_threshold_lookup);
    tuple_case!(capture_count_5, build_capture_threshold(5), dynamic_capture_threshold_lookup);

    macro_rules! raw_case {
        ($name:ident, $setup:ident, $run:ident) => {
            #[library_benchmark]
            #[bench::run($setup())]
            fn $name(router: RawResolver) -> RawResolver {
                $run(&router);
                router
            }
        };
    }

    raw_case!(misses_early, build_dynamic_misses, dynamic_early_miss);
    raw_case!(misses_late, build_dynamic_misses, dynamic_late_miss);
    raw_case!(misses_wrong_method, build_dynamic_misses, dynamic_wrong_method);

    raw_case!(features_rest, build_dynamic_features, dynamic_rest);
    raw_case!(features_affix, build_dynamic_features, dynamic_affix);
    raw_case!(features_no_verb_table, build_dynamic_no_verb, dynamic_no_verb);
    raw_case!(
        features_verb_table_nonverb_hit,
        build_dynamic_with_verb,
        dynamic_with_verb_nonverb_hit
    );
    raw_case!(features_verb_hit, build_dynamic_with_verb, dynamic_verb_hit);

    tuple_case!(
        segment_depth_shallow_in_16_table,
        build_dynamic_depth(16),
        dynamic_depth_table_shallow_lookup
    );
    tuple_case!(
        segment_depth_shallow_in_17_table,
        build_dynamic_depth(17),
        dynamic_depth_table_shallow_lookup
    );
    tuple_case!(segment_depth_deep_16, build_dynamic_depth(16), dynamic_depth_table_deep_lookup);
    tuple_case!(segment_depth_deep_17, build_dynamic_depth(17), dynamic_depth_table_deep_lookup);

    #[library_benchmark]
    #[bench::run(build_deep_dynamic())]
    fn deep_scratch_shallow(router: RawResolver) -> RawResolver {
        dynamic_deep_table_shallow_lookup(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_deep_dynamic())]
    fn deep_scratch_deep(router: RawResolver) -> RawResolver {
        dynamic_deep_table_deep_lookup(&router);
        router
    }

    library_benchmark_group!(
        name = typed;
        benchmarks = typed_unit, typed_parse, typed_owned_plain, typed_owned_percent
    );
    library_benchmark_group!(
        name = fanout;
        benchmarks = fanout_01, fanout_02, fanout_04, fanout_08, fanout_16, fanout_32, fanout_64
    );
    library_benchmark_group!(
        name = capture_count;
        benchmarks = capture_count_4, capture_count_5
    );
    library_benchmark_group!(
        name = misses;
        benchmarks = misses_early, misses_late, misses_wrong_method
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
        name = segment_depth;
        benchmarks =
            segment_depth_shallow_in_16_table,
            segment_depth_shallow_in_17_table,
            segment_depth_deep_16,
            segment_depth_deep_17
    );
    library_benchmark_group!(
        name = deep_scratch;
        benchmarks = deep_scratch_shallow, deep_scratch_deep
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{capture_count, deep_scratch, fanout, features, misses, segment_depth, typed};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes", "--cache-sim=yes"]));
    library_benchmark_groups =
        typed, fanout, capture_count, misses, features, segment_depth, deep_scratch
);
