// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind comparisons for `JsonBody` and `FormBody` construction.
//!
//! Paired with `routerama_typed_body.rs`.

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

    include!("common/typed_body_scenarios.rs");

    #[library_benchmark]
    #[bench::run(prepare_flat())]
    fn construction_json_flat(prepared: PreparedFlat) -> Observation {
        std::hint::black_box(run_json_flat_prepared(prepared))
    }

    #[library_benchmark]
    #[bench::run(prepare_flat())]
    fn construction_form_flat(prepared: PreparedFlat) -> Observation {
        std::hint::black_box(run_form_flat_prepared(prepared))
    }

    #[library_benchmark]
    #[bench::run(prepare_escaped())]
    fn construction_json_escaped(prepared: PreparedEscaped) -> Observation {
        std::hint::black_box(run_json_escaped_prepared(prepared))
    }

    #[library_benchmark]
    #[bench::run(prepare_escaped())]
    fn construction_form_escaped(prepared: PreparedEscaped) -> Observation {
        std::hint::black_box(run_form_escaped_prepared(prepared))
    }

    #[library_benchmark]
    #[bench::run(prepare_large())]
    fn construction_json_large(prepared: PreparedLarge) -> Observation {
        std::hint::black_box(run_json_large_prepared(prepared))
    }

    #[library_benchmark]
    #[bench::run(prepare_large())]
    fn construction_form_large(prepared: PreparedLarge) -> Observation {
        std::hint::black_box(run_form_large_prepared(prepared))
    }

    library_benchmark_group!(
        name = construction;
        benchmarks =
            construction_json_flat,
            construction_form_flat,
            construction_json_escaped,
            construction_form_escaped,
            construction_json_large,
            construction_form_large
    );
}

#[cfg(target_os = "linux")]
pub use linux::construction;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = construction);
