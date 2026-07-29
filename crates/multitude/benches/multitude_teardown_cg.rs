// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for releasing allocations.
//!
//! Paired with `multitude_teardown.rs`, which measures the same hot paths
//! under Criterion.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
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
#[path = "multitude_teardown/shared.rs"]
mod shared;

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::{library_benchmark, library_benchmark_group};

    use crate::shared::{
        LARGE, MEDIUM, SMALL, StandardState, bumpalo_state, free_standard, multitude_state, reset_bumpalo, reset_multitude, standard_state,
    };

    macro_rules! benchmark_count {
        ($standard:ident, $multitude:ident, $bumpalo:ident, $count:ident) => {
            #[library_benchmark]
            #[bench::run(&mut standard_state::<$count>())]
            fn $standard(state: &mut StandardState<$count>) {
                free_standard(state);
            }

            #[library_benchmark]
            #[bench::run(&mut multitude_state::<$count>())]
            fn $multitude(arena: &mut multitude::Arena) {
                reset_multitude(arena);
            }

            #[library_benchmark]
            #[bench::run(&mut bumpalo_state::<$count>())]
            fn $bumpalo(bump: &mut bumpalo::Bump) {
                reset_bumpalo(bump);
            }
        };
    }

    benchmark_count!(free_1_standard, free_1_multitude, free_1_bumpalo, SMALL);
    benchmark_count!(free_32_standard, free_32_multitude, free_32_bumpalo, MEDIUM);
    benchmark_count!(free_1000_standard, free_1000_multitude, free_1000_bumpalo, LARGE);

    library_benchmark_group!(
        name = free_1;
        benchmarks = free_1_standard, free_1_multitude, free_1_bumpalo
    );
    library_benchmark_group!(
        name = free_32;
        benchmarks = free_32_standard, free_32_multitude, free_32_bumpalo
    );
    library_benchmark_group!(
        name = free_1000;
        benchmarks = free_1000_standard, free_1000_multitude, free_1000_bumpalo
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{free_1, free_32, free_1000};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = free_1, free_32, free_1000
);
