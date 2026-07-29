// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for Serde deserialization in the `multitude` crate.
//!
//! Paired with `multitude_serde.rs`, which covers the same operations under
//! wall-clock measurement.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(dead_code, reason = "deserialized benchmark records are consumed as whole values")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "Gungraun benchmark inputs are passed by value by the framework"
)]
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
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
#[path = "multitude_serde/shared.rs"]
mod shared;

#[cfg(target_os = "linux")]
mod linux {
    use bumpalo::Bump;
    use gungraun::{library_benchmark, library_benchmark_group};
    use multitude::Arena;
    use multitude::de::Value;

    use crate::shared::{
        ArenaOutput, ArenaRecord, StandardRecord, arena_output, batch_bumpalo_lifecycle, batch_multitude_lifecycle,
        batch_standard_lifecycle, dynamic_arena_hot_path, dynamic_standard_hot_path, typed_arena_hot_path, typed_bumpalo_lifecycle,
        typed_multitude_lifecycle, typed_standard_hot_path, typed_standard_lifecycle, warm_bump, warm_reset_arena,
    };

    #[library_benchmark]
    #[bench::run(&mut arena_output())]
    fn typed_arena_owned(output: &mut ArenaOutput<ArenaRecord>) {
        typed_arena_hot_path(output);
    }

    #[library_benchmark]
    #[bench::run(&mut None)]
    fn typed_serde_json_owned(output: &mut Option<StandardRecord>) {
        typed_standard_hot_path(output);
    }

    #[library_benchmark]
    #[bench::run(&mut arena_output())]
    fn dynamic_arena_value(output: &mut ArenaOutput<Value>) {
        dynamic_arena_hot_path(output);
    }

    #[library_benchmark]
    #[bench::run(&mut None)]
    fn dynamic_serde_json_value(output: &mut Option<serde_json::Value>) {
        dynamic_standard_hot_path(output);
    }

    #[library_benchmark]
    #[bench::run(&mut ())]
    fn lifecycle_serde_json(state: &mut ()) {
        typed_standard_lifecycle(state);
    }

    #[library_benchmark]
    #[bench::run(&mut warm_reset_arena())]
    fn lifecycle_multitude(arena: &mut Arena) {
        typed_multitude_lifecycle(arena);
    }

    #[library_benchmark]
    #[bench::run(&mut warm_bump())]
    fn lifecycle_bumpalo(bump: &mut Bump) {
        typed_bumpalo_lifecycle(bump);
    }

    #[library_benchmark]
    #[bench::run(&mut ())]
    fn batch_lifecycle_serde_json(state: &mut ()) {
        batch_standard_lifecycle(state);
    }

    #[library_benchmark]
    #[bench::run(&mut warm_reset_arena())]
    fn batch_lifecycle_multitude(arena: &mut Arena) {
        batch_multitude_lifecycle(arena);
    }

    #[library_benchmark]
    #[bench::run(&mut warm_bump())]
    fn batch_lifecycle_bumpalo(bump: &mut Bump) {
        batch_bumpalo_lifecycle(bump);
    }

    library_benchmark_group!(
        name = typed;
        benchmarks = typed_arena_owned, typed_serde_json_owned
    );
    library_benchmark_group!(
        name = dynamic;
        benchmarks = dynamic_arena_value, dynamic_serde_json_value
    );
    library_benchmark_group!(
        name = typed_lifecycle;
        benchmarks = lifecycle_serde_json, lifecycle_multitude, lifecycle_bumpalo
    );
    library_benchmark_group!(
        name = batch_lifecycle;
        benchmarks = batch_lifecycle_serde_json, batch_lifecycle_multitude, batch_lifecycle_bumpalo
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{batch_lifecycle, dynamic, typed, typed_lifecycle};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = typed, dynamic, typed_lifecycle, batch_lifecycle
);
