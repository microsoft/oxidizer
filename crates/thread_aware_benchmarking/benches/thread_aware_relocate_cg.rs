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
//! cannot model. What this file does catch is the extra work a cross-key miss
//! pays over a hit — the re-probe load, the factory call, and the two cell
//! writes — and any future growth of either branch. Storage is populated before
//! timing, so its initial setup stays out of the counts.
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
    use thread_aware::thread::ThreadBuilder;
    use thread_aware::{Arc, PerThread, Thread, ThreadAware};
    use thread_aware_benchmarking::{Payload, Tree};

    fn threads(count: usize) -> Vec<Thread> {
        let builder = ThreadBuilder::default();
        (0..count)
            .map(|_| {
                let builder = builder.clone();
                std::thread::spawn(move || builder.build(std::thread::current().id()))
                    .join()
                    .unwrap()
            })
            .collect()
    }

    // Destination thread already holds a value.
    fn materialized() -> (Arc<Payload, PerThread>, Thread, Thread) {
        let threads = threads(2);
        let mut arc = Arc::<Payload, PerThread>::new(Payload::new);

        for thread in &threads {
            let mut probe = arc.clone();
            probe.relocate(None, thread);
        }

        arc.relocate(None, &threads[0]);
        (arc, threads[0].clone(), threads[1].clone())
    }

    // Destination thread holds the value carried by the returned Arc.
    fn materialized_at_destination() -> (Arc<Payload, PerThread>, Thread, Thread) {
        let (mut arc, source, destination) = materialized();
        arc.relocate(Some(&source), &destination);

        (arc, destination.clone(), destination)
    }

    // A primer thread populates shared storage before timing; the source and destination keys stay
    // empty, so the timed relocation is a genuine cross-key miss that materializes the destination
    // and records the carried value under the empty source key.
    fn empty() -> (Arc<Payload, PerThread>, Thread, Thread) {
        let threads = threads(3);
        let arc = Arc::<Payload, PerThread>::new(Payload::new);

        let mut seed = arc.clone();
        seed.relocate(None, &threads[0]);

        (arc, threads[1].clone(), threads[2].clone())
    }

    // Every layer of the tree already holds a value for both threads.
    fn materialized_tree() -> (Tree, Thread, Thread) {
        let threads = threads(2);
        let tree = Tree::new();
        let mut ids = Vec::new();

        for thread in &threads {
            let mut probe = tree.clone();
            probe.relocate(None, thread);
            ids.extend(probe.leaf_ids());
        }

        // Proves the tree really has one independent storage map per layer, which
        // is the only reason its instruction count differs from the bare payload.
        ids.sort_unstable();
        let distinct = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), distinct, "every layer of every thread partition must hold its own value");

        let mut tree = tree;
        tree.relocate(None, &threads[0]);
        (tree, threads[0].clone(), threads[1].clone())
    }

    // Every layer's storage is populated by the primer relocation, but no layer
    // holds a value for the source or destination threads yet.
    fn empty_tree() -> (Tree, Thread, Thread) {
        let threads = threads(3);
        let tree = Tree::new();

        let mut seed = tree.clone();
        seed.relocate(None, &threads[0]);

        (tree, threads[1].clone(), threads[2].clone())
    }

    #[library_benchmark]
    #[bench::run(materialized())]
    fn hit_path_pre_materialized(input: (Arc<Payload, PerThread>, Thread, Thread)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(&source)), black_box(&destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(materialized_at_destination())]
    fn hit_path_same_thread(input: (Arc<Payload, PerThread>, Thread, Thread)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(&source)), black_box(&destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(empty())]
    fn miss_path_new_thread(input: (Arc<Payload, PerThread>, Thread, Thread)) -> u64 {
        let (mut arc, source, destination) = input;

        arc.relocate(black_box(Some(&source)), black_box(&destination));

        black_box(arc.id)
    }

    #[library_benchmark]
    #[bench::run(materialized_tree())]
    fn hit_path_tree(input: (Tree, Thread, Thread)) -> u64 {
        let (mut tree, source, destination) = input;

        tree.relocate(black_box(Some(&source)), black_box(&destination));

        black_box(tree.leaf_id())
    }

    #[library_benchmark]
    #[bench::run(empty_tree())]
    fn miss_path_tree(input: (Tree, Thread, Thread)) -> u64 {
        let (mut tree, source, destination) = input;

        tree.relocate(black_box(Some(&source)), black_box(&destination));

        black_box(tree.leaf_id())
    }

    library_benchmark_group!(
        name = hit_path;
        benchmarks = hit_path_pre_materialized, hit_path_same_thread, hit_path_tree
    );

    library_benchmark_group!(
        name = miss_path;
        benchmarks = miss_path_new_thread, miss_path_tree
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
