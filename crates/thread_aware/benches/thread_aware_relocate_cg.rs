// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `thread_aware::Arc<T, S>::relocate`.
//!
//! Paired with `thread_aware_relocate.rs`, which covers the same operations
//! under wall-clock measurement. Only the uncontended `hit_path` and
//! `miss_path` subgroups appear here: the `storm` and `handoff` subgroups
//! measure lock contention across threads, which the single-threaded Callgrind
//! simulator cannot model.
//!
//! The instruction counts here are a regression guard, not a demonstration of
//! the shared-lock probe. An uncontended shared acquisition and an uncontended
//! exclusive acquisition cost nearly the same number of instructions, so the
//! benefit of the probe only appears under contention, which the simulator
//! cannot model. What this file does catch is the extra probe the miss path
//! pays, and any future growth of either branch.
//!
//! Run with: `cargo bench -p thread_aware --bench thread_aware_relocate_cg`
//! on a Linux host with Valgrind installed.

#![allow(missing_docs, reason = "benchmark code")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
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

// Gungraun requires Valgrind, which is Linux-only. On other platforms this
// bench target compiles to a no-op so `cargo build --all-targets` still works.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::sync::atomic::{AtomicU64, Ordering};

    use gungraun::{library_benchmark, library_benchmark_group};
    use thread_aware::affinity::{Affinity, pinned_affinities};
    use thread_aware::{Arc, PerCore, ThreadAware};

    static NEXT_PAYLOAD_ID: AtomicU64 = AtomicU64::new(0);

    // Mirrors the payload used by the criterion counterpart so the two files
    // measure the same object.
    #[derive(Debug)]
    pub(crate) struct Payload {
        id: u64,
    }

    impl Payload {
        fn new() -> Self {
            Self {
                id: NEXT_PAYLOAD_ID.fetch_add(1, Ordering::Relaxed),
            }
        }
    }

    fn affinities() -> Vec<Affinity> {
        pinned_affinities(&[2])
    }

    // Destination affinity already holds a value.
    fn materialized() -> (Arc<Payload, PerCore>, Affinity, Affinity) {
        let affinities = affinities();
        let arc = Arc::<Payload, PerCore>::new(Payload::new);

        for &affinity in &affinities {
            let mut probe = arc.clone();
            probe.relocate(None, affinity);
        }

        (arc, affinities[0], affinities[1])
    }

    // Destination affinity is empty, so the value has to be materialized.
    fn empty() -> (Arc<Payload, PerCore>, Affinity) {
        let affinities = affinities();

        (Arc::<Payload, PerCore>::new(Payload::new), affinities[1])
    }

    #[library_benchmark]
    #[bench::run(materialized())]
    fn hit_path_pre_materialized(input: (Arc<Payload, PerCore>, Affinity, Affinity)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(source)), black_box(destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(materialized())]
    fn hit_path_same_affinity(input: (Arc<Payload, PerCore>, Affinity, Affinity)) -> u64 {
        let (mut arc, _source, destination) = input;

        arc.relocate(black_box(Some(destination)), black_box(destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(empty())]
    fn miss_path_new_affinity(input: (Arc<Payload, PerCore>, Affinity)) -> u64 {
        let (mut arc, destination) = input;

        arc.relocate(black_box(None), black_box(destination));

        black_box(arc.id)
    }

    library_benchmark_group!(
        name = hit_path;
        benchmarks = hit_path_pre_materialized, hit_path_same_affinity
    );

    library_benchmark_group!(
        name = miss_path;
        benchmarks = miss_path_new_affinity
    );
}

#[cfg(target_os = "linux")]
pub use linux::{hit_path, miss_path};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = gungraun::LibraryBenchmarkConfig::default()
        .tool(gungraun::Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = hit_path, miss_path
);
