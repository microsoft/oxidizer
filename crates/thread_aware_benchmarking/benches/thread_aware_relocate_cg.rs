// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `thread_aware::Arc<T, S>::relocate`.
//!
//! Paired with `thread_aware_relocate.rs`, which covers the same operations
//! under wall-clock measurement. Only the uncontended `hit_path` and `miss_path`
//! subgroups appear here: the `concurrent` subgroup measures scaling across
//! threads, which the single-threaded Callgrind simulator cannot model.
//!
//! The instruction counts here are a regression guard, not a demonstration of
//! the concurrency win. Single-threaded, a hit is a cheap lock-free read and a miss
//! adds a second load, the factory call, and the write-once publish; those cost
//! nearly the same whether or not other threads are relocating, so the benefit
//! of the lock-free cells only appears under contention, which the simulator
//! cannot model. What this file does catch is the extra work a cross-slot miss
//! pays over a hit — the re-probe load, the factory call, and the two cell
//! writes — and any future growth of either branch. The slot table is sized
//! before timing, so the one-time table allocation stays out of the counts.
//!
//! Run with: `cargo bench -p thread_aware_benchmarking --bench thread_aware_relocate_cg`
//! on a Linux host with Valgrind installed.

#![allow(missing_docs, reason = "benchmark code")]
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

    use gungraun::{library_benchmark, library_benchmark_group};
    use thread_aware::affinity::{Affinity, pinned_affinities};
    use thread_aware::{Arc, PerCore, ThreadAware};
    use thread_aware_benchmarking::{Payload, Tree};

    fn affinities() -> Vec<Affinity> {
        pinned_affinities(&[2])
    }

    // Destination affinity already holds a value.
    fn materialized() -> (Arc<Payload, PerCore>, Affinity, Affinity) {
        let affinities = affinities();
        let mut arc = Arc::<Payload, PerCore>::new(Payload::new);

        for &affinity in &affinities {
            let mut probe = arc.clone();
            probe.relocate(None, affinity);
        }

        arc.relocate(None, affinities[0]);
        (arc, affinities[0], affinities[1])
    }

    // Destination affinity holds the value carried by the returned Arc.
    fn materialized_at_destination() -> (Arc<Payload, PerCore>, Affinity, Affinity) {
        let (mut arc, source, destination) = materialized();
        arc.relocate(Some(source), destination);

        (arc, destination, destination)
    }

    // A primer affinity sizes the shared slot table before timing; the source and
    // destination slots stay empty, so the timed relocation is a genuine cross-slot
    // miss that materializes the destination and records the carried value in the
    // empty source, without the one-time table allocation.
    fn empty() -> (Arc<Payload, PerCore>, Affinity, Affinity) {
        let affinities = pinned_affinities(&[3]);
        let arc = Arc::<Payload, PerCore>::new(Payload::new);

        let mut seed = arc.clone();
        seed.relocate(None, affinities[0]);

        (arc, affinities[1], affinities[2])
    }

    // Every layer of the tree already holds a value for both affinities.
    fn materialized_tree() -> (Tree, Affinity, Affinity) {
        let affinities = affinities();
        let tree = Tree::new();
        let mut ids = Vec::new();

        for &affinity in &affinities {
            let mut probe = tree.clone();
            probe.relocate(None, affinity);
            ids.extend(probe.leaf_ids());
        }

        // Proves the tree really has one independent slot table per layer, which
        // is the only reason its instruction count differs from the bare payload.
        ids.sort_unstable();
        let distinct = ids.len();
        ids.dedup();
        assert_eq!(
            ids.len(),
            distinct,
            "every layer of every affinity's partition must hold its own value"
        );

        let mut tree = tree;
        tree.relocate(None, affinities[0]);
        (tree, affinities[0], affinities[1])
    }

    // Every layer's slot table is sized by the primer relocation, but no layer
    // holds a value for the source or destination affinities yet.
    fn empty_tree() -> (Tree, Affinity, Affinity) {
        let affinities = pinned_affinities(&[3]);
        let tree = Tree::new();

        let mut seed = tree.clone();
        seed.relocate(None, affinities[0]);

        (tree, affinities[1], affinities[2])
    }

    #[library_benchmark]
    #[bench::run(materialized())]
    fn hit_path_pre_materialized(input: (Arc<Payload, PerCore>, Affinity, Affinity)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(source)), black_box(destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(materialized_at_destination())]
    fn hit_path_same_affinity(input: (Arc<Payload, PerCore>, Affinity, Affinity)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(source)), black_box(destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(empty())]
    fn miss_path_new_affinity(input: (Arc<Payload, PerCore>, Affinity, Affinity)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(source)), black_box(destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(materialized_tree())]
    fn hit_path_tree(input: (Tree, Affinity, Affinity)) -> u64 {
        let (mut tree, source, destination) = input;

        tree.relocate(black_box(Some(source)), black_box(destination));

        black_box(tree.leaf_id())
    }

    #[library_benchmark]
    #[bench::run(empty_tree())]
    fn miss_path_tree(input: (Tree, Affinity, Affinity)) -> u64 {
        let (mut tree, source, destination) = input;

        tree.relocate(black_box(Some(source)), black_box(destination));

        black_box(tree.leaf_id())
    }

    library_benchmark_group!(
        name = hit_path;
        benchmarks = hit_path_pre_materialized, hit_path_same_affinity, hit_path_tree
    );

    library_benchmark_group!(
        name = miss_path;
        benchmarks = miss_path_new_affinity, miss_path_tree
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
