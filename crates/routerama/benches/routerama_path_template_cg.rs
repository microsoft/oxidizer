// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `PathTemplate::parse` across template shapes.
//!
//! Paired with `routerama_path_template.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports two harnesses")]
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
mod linux {
    use gungraun::prelude::*;

    include!("common/path_template_scenarios.rs");

    macro_rules! parse_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $name(prepared: PreparedScenario) -> Summary {
                std::hint::black_box(run_parse_prepared(&prepared))
            }
        };
    }

    parse_case!(parse_literal, Literal);
    parse_case!(parse_captures, Captures);
    parse_case!(parse_affix, Affix);
    parse_case!(parse_deep, Deep);

    library_benchmark_group!(
        name = parse;
        benchmarks = parse_literal, parse_captures, parse_affix, parse_deep
    );
}

#[cfg(target_os = "linux")]
pub use linux::parse;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = parse);
