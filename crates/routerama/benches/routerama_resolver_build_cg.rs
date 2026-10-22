// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `RawResolver::new` and `RawResolver::new_checked`
//! across route-table sizes and shapes.
//!
//! Paired with `routerama_resolver_build.rs`.

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

    include!("common/resolver_build_scenarios.rs");

    macro_rules! build_case {
        ($name:ident, $shape:ident, $size:literal) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario { shape: Shape::$shape, size: $size }))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_new_prepared(prepared))
            }
        };
    }

    macro_rules! build_checked_case {
        ($name:ident, $shape:ident, $size:literal) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario { shape: Shape::$shape, size: $size }))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_new_checked_prepared(prepared))
            }
        };
    }

    build_case!(new_literal_0016, Literal, 16);
    build_case!(new_literal_0128, Literal, 128);
    build_case!(new_literal_1024, Literal, 1024);
    build_case!(new_captures_0016, Captures, 16);
    build_case!(new_captures_0128, Captures, 128);
    build_case!(new_captures_1024, Captures, 1024);

    build_checked_case!(new_checked_literal_0016, Literal, 16);
    build_checked_case!(new_checked_literal_0128, Literal, 128);
    build_checked_case!(new_checked_literal_1024, Literal, 1024);
    build_checked_case!(new_checked_captures_0016, Captures, 16);
    build_checked_case!(new_checked_captures_0128, Captures, 128);
    build_checked_case!(new_checked_captures_1024, Captures, 1024);

    library_benchmark_group!(
        name = new;
        benchmarks =
            new_literal_0016,
            new_literal_0128,
            new_literal_1024,
            new_captures_0016,
            new_captures_0128,
            new_captures_1024
    );

    library_benchmark_group!(
        name = new_checked;
        benchmarks =
            new_checked_literal_0016,
            new_checked_literal_0128,
            new_checked_literal_1024,
            new_checked_captures_0016,
            new_checked_captures_0128,
            new_checked_captures_1024
    );
}

#[cfg(target_os = "linux")]
pub use linux::{new, new_checked};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = new, new_checked);
