// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock benchmarks for `thread_aware::Arc<T, S>::relocate`.
//!
//! `relocate` is the hot path of the crate: a thread-per-core runtime calls it
//! once per cross-thread spawn, for every `Arc<_, PerThread>` reachable in the
//! relocated object graph. The steady state is that the destination thread
//! already holds its value, so the call reduces to cloning a `sync::Arc` out of
//! a keyed storage entry.
//!
//! The following subjects are relocated throughout:
//!
//! * A bare `Arc<Payload, PerThread>`, which isolates the cost of a single
//!   relocation.
//! * A five-layer object tree, which is what actually crosses threads in
//!   practice. Relocating it walks the whole graph and reads one storage map per
//!   layer, so it reports what storage access costs per message rather than per
//!   call.
//!
//! The suite covers these shapes:
//!
//! * `hit_path` / `miss_path` — uncontended cost of the two branches.
//! * `concurrent` — the hit-path cost with as many concurrent workers as
//!   processors, and beyond, each relocating into its own key, so there is no
//!   shared cell to contend on.
//!
//! Paired with `thread_aware_relocate_cg.rs`, which covers `hit_path` and
//! `miss_path` under instruction-count measurement. The `concurrent` subgroup has
//! no Callgrind counterpart because it measures scaling across threads, which the
//! single-threaded simulator cannot model.
//!
//! Run with: `cargo bench -p thread_aware_benchmarking --bench thread_aware_relocate`

#![allow(clippy::unwrap_used, reason = "benchmark code")]

use std::hint::black_box;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{sync, thread};

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use thread_aware::thread::ThreadBuilder;
use thread_aware::{Arc, PerThread, Thread, ThreadAware};
use thread_aware_benchmarking::{Payload, TREE_DEPTH, Tree};

/// How far the oversubscribed case of the `concurrent` group exceeds the
/// processor count.
///
/// Oversubscription is the point of that shape: it puts more runnable workers in
/// the scheduler queue than there are processors to run them, the regime a
/// thread-per-core runtime reaches whenever it has more runnable work than cores.
/// Every worker still relocates between its own already-filled source and
/// destination keys, so no two workers share a cell; the shape exposes how the
/// uncontended hit path behaves under scheduler pressure, not cell contention.
///
/// A small multiple is enough to reach that regime. Higher factors only add more
/// scheduler queueing without exercising a different code path.
const CONCURRENT_OVERSUBSCRIPTION: usize = 2;

fn threads(count: usize) -> Vec<Thread> {
    let builder = ThreadBuilder::default();
    (0..count)
        .map(|_| {
            let builder = builder.clone();
            thread::spawn(move || builder.build(thread::current().id())).join().unwrap()
        })
        .collect()
}

/// Builds an `Arc<Payload, PerThread>` whose key is already materialized for every
/// thread in `threads`.
///
/// Every benchmark that measures the hit path needs this, because a key is only
/// filled by a relocation that misses first.
fn materialized(threads: &[Thread]) -> Arc<Payload, PerThread> {
    let arc = Arc::<Payload, PerThread>::new(Payload::new);

    let mut ids = Vec::with_capacity(threads.len());

    for thread in threads {
        let mut probe = arc.clone();
        probe.relocate(None, thread);
        ids.push(probe.id);
    }

    ids.sort_unstable();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), distinct, "every thread partition must hold its own value");

    arc
}

/// Populates the shared storage of `subject` only for `primer`'s key.
///
/// Relocating a throwaway clone into a primer thread materializes that key in
/// every storage map the subject reaches, while leaving every other key empty.
/// Timing a relocation between two thread coordinates distinct from `primer`
/// therefore measures a genuine cross-key miss with storage already initialized.
fn seed_storage<T: ThreadAware + Clone>(subject: &T, primer: &Thread) {
    let mut seed = subject.clone();
    seed.relocate(None, primer);
}

/// Builds a [`Tree`] whose every layer is already materialized for every thread
/// in `threads`.
fn materialized_tree(threads: &[Thread]) -> Tree {
    let tree = Tree::new();

    assert_eq!(tree.node_count(), TREE_DEPTH, "the tree must be as deep as it claims");

    let mut ids = Vec::with_capacity(threads.len().saturating_mul(TREE_DEPTH));

    for thread in threads {
        let mut probe = tree.clone();
        probe.relocate(None, thread);
        ids.extend(probe.leaf_ids());
    }

    ids.sort_unstable();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), distinct, "every layer of every thread partition must hold its own value");

    tree
}

// =========================================================================
// hit_path — destination thread already holds a value.
// =========================================================================

fn bench_hit_path(c: &mut Criterion) {
    let threads = threads(2);
    let (source, destination) = (&threads[0], &threads[1]);
    let arc = materialized(&threads);

    let mut group = c.benchmark_group("thread_aware_relocate/hit_path");

    group.bench_function("pre_materialized", |b| {
        b.iter_batched(
            || {
                let mut subject = arc.clone();
                subject.relocate(None, source);
                subject
            },
            |mut subject| {
                black_box(&mut subject).relocate(black_box(Some(source)), black_box(destination));
                subject
            },
            BatchSize::SmallInput,
        );
    });

    let mut subject = arc;
    subject.relocate(None, destination);
    group.bench_function("same_thread", |b| {
        b.iter(|| {
            black_box(&mut subject).relocate(black_box(Some(destination)), black_box(destination));
        });
    });

    let tree = materialized_tree(&threads);
    group.bench_function("tree", |b| {
        b.iter_batched(
            || {
                let mut subject = tree.clone();
                subject.relocate(None, source);
                subject
            },
            |mut subject| {
                black_box(&mut subject).relocate(black_box(Some(source)), black_box(destination));
                subject
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// =========================================================================
// miss_path — destination key is empty, so the value is materialized.
//             A primer thread populates the shared storage before timing, so
//             the measurement is the cross-key miss itself — the re-probe
//             load, the factory call, and the two cell writes — rather than
//             initial storage setup.
// =========================================================================

fn bench_miss_path(c: &mut Criterion) {
    let threads = threads(3);
    let (primer, source, destination) = (&threads[0], &threads[1], &threads[2]);

    let mut group = c.benchmark_group("thread_aware_relocate/miss_path");

    group.bench_function("new_thread", |b| {
        b.iter_batched(
            || {
                let arc = Arc::<Payload, PerThread>::new(Payload::new);
                seed_storage(&arc, primer);
                arc
            },
            |mut arc| {
                arc.relocate(black_box(Some(source)), black_box(destination));
                arc
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("tree", |b| {
        b.iter_batched(
            || {
                let tree = Tree::new();
                seed_storage(&tree, primer);
                tree
            },
            |mut tree| {
                tree.relocate(black_box(Some(source)), black_box(destination));
                tree
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// =========================================================================
// concurrent — the hit-path cost while many workers relocate into their own
//              keys at once, with no shared cell to contend on.
// =========================================================================

/// Persistent worker pool that measures a relocation performed concurrently from
/// every worker, each into its own already-materialized destination key.
///
/// The workers are created once and reused for every Criterion sample; spawning
/// them per sample would cost orders of magnitude more than the work measured.
///
/// A round hands every worker a batch of relocations, releases them together, and
/// measures the wall-clock time until the last one finishes. The batch is what
/// makes the measurement possible. A single relocation is a handful of
/// nanoseconds, while releasing the workers and waking them onto their cores is
/// tens of microseconds, so a round must contain enough relocations to reduce that
/// fixed cost to a negligible fraction of the sample. Criterion drives the batch
/// size up until a sample fills its target time and fits round duration against
/// batch size, so the fixed release cost lands in the regression intercept and the
/// reported per-iteration time is the batch makespan divided by the batch size: the
/// amortized cost of one relocation on the worker that finishes last. Because the
/// destinations are distinct and already populated, no two workers share a keyed
/// cell, so that figure reflects the hit path while every worker is busy.
///
/// Readiness is proven before the clock starts. Every worker parks on `ready`,
/// the controller waits there too, and only once all of them have arrived does it
/// start the clock and release `start`. Timing therefore excludes the time spent
/// waiting for the slowest workers to reach the barrier.
struct ConcurrentRelocation {
    ready: sync::Arc<Barrier>,
    start: sync::Arc<Barrier>,
    end: sync::Arc<Barrier>,
    batch: sync::Arc<AtomicU64>,
    shutdown: sync::Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl ConcurrentRelocation {
    /// Creates a pool of `thread_count` workers, each relocating its own clone of
    /// the subject that `make` materializes across all participating threads.
    fn new<T>(thread_count: usize, make: impl FnOnce(&[Thread]) -> T) -> Self
    where
        T: ThreadAware + Clone + Send + 'static,
    {
        // Give every worker distinct source and destination partitions. Two partitions per worker
        // keep both directions of the repeated handoff uncontended by other workers.
        let coordinate_count = thread_count.saturating_mul(2);
        let threads = threads(coordinate_count);
        let subject = make(&threads);

        let ready = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let start = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let end = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let batch = sync::Arc::new(AtomicU64::new(0));
        let shutdown = sync::Arc::new(AtomicBool::new(false));

        let workers = (0..thread_count)
            .map(|index| {
                let source = threads[index].clone();
                let destination = threads[index.saturating_add(thread_count)].clone();
                let mut subject = subject.clone();
                subject.relocate(None, &source);

                let ready = sync::Arc::clone(&ready);
                let start = sync::Arc::clone(&start);
                let end = sync::Arc::clone(&end);
                let batch = sync::Arc::clone(&batch);
                let shutdown = sync::Arc::clone(&shutdown);

                thread::spawn(move || {
                    let mut at_source = true;

                    loop {
                        ready.wait();
                        start.wait();

                        if shutdown.load(Ordering::Acquire) {
                            break;
                        }

                        let iterations = batch.load(Ordering::Acquire);
                        let pairs = iterations / 2;

                        // Alternate between two valid populated partitions. Unrolling pairs keeps
                        // direction selection outside the per-relocation hot path; only an odd tail
                        // changes which direction begins the next batch.
                        if at_source {
                            for _ in 0..pairs {
                                subject.relocate(Some(&source), &destination);
                                black_box(&subject);
                                subject.relocate(Some(&destination), &source);
                                black_box(&subject);
                            }

                            if !iterations.is_multiple_of(2) {
                                subject.relocate(Some(&source), &destination);
                                black_box(&subject);
                                at_source = false;
                            }
                        } else {
                            for _ in 0..pairs {
                                subject.relocate(Some(&destination), &source);
                                black_box(&subject);
                                subject.relocate(Some(&source), &destination);
                                black_box(&subject);
                            }

                            if !iterations.is_multiple_of(2) {
                                subject.relocate(Some(&destination), &source);
                                black_box(&subject);
                                at_source = true;
                            }
                        }

                        end.wait();
                    }
                })
            })
            .collect();

        Self {
            ready,
            start,
            end,
            batch,
            shutdown,
            workers,
        }
    }

    /// Runs one round of `batch` relocations per worker and returns the wall-clock
    /// time until the last worker finished.
    ///
    /// Dividing by `batch` gives the amortized cost of one relocation on the
    /// worker that finishes last; workers are synchronized once per batch, not per
    /// relocation. Criterion does that division, and its regression discards the
    /// fixed per-round release cost as the intercept.
    fn run(&self, batch: u64) -> Duration {
        self.batch.store(batch, Ordering::Release);

        // Wait for every worker to park before starting the clock, so the timed
        // region is the relocation work rather than the barrier arrival skew.
        self.ready.wait();

        let started = Instant::now();
        self.start.wait();
        self.end.wait();
        started.elapsed()
    }
}

impl Drop for ConcurrentRelocation {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        // Release the workers from both barriers so they observe the shutdown flag
        // and leave the loop. They do not reach `end` on this pass, so the
        // controller must not wait on it.
        self.ready.wait();
        self.start.wait();

        for worker in self.workers.drain(..) {
            worker.join().unwrap();
        }
    }
}

fn bench_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("thread_aware_relocate/concurrent");

    let saturated = SystemHardware::current().processors().len();

    // A sweep over worker count: one worker per processor, then more workers than
    // processors. Every worker relocates into its own distinct, already-populated
    // key, so the measured relocations are all hits and no two workers share a
    // cell; the reported per-relocation time therefore reflects the hit path scaling
    // as the machine fills rather than contention. The oversubscribed shape
    // adds scheduler pressure, not contention. The uncontended single-worker
    // cost belongs to `hit_path`, so it is deliberately absent here: one worker would
    // only add this harness's thread-handoff overhead to the same number.
    //
    // The subject is the object tree, which is what actually crosses threads in
    // a consumer. It reads one storage map per layer, so each message does several
    // cell reads and the per-message storage cost is large enough to resolve; a
    // bare `Arc` does one read, too little to resolve above the harness noise.
    let shapes = [
        ("threads_saturated", saturated),
        ("threads_oversubscribed", saturated * CONCURRENT_OVERSUBSCRIPTION),
    ];

    for (name, thread_count) in shapes {
        // One relocation per worker per counted element, so Criterion reports
        // aggregate relocation throughput alongside the per-relocation time.
        group.throughput(Throughput::Elements(thread_count as u64));

        let pool = ConcurrentRelocation::new(thread_count, materialized_tree);

        group.bench_function(name, |b| {
            b.iter_custom(|batch| pool.run(batch));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_hit_path, bench_miss_path, bench_concurrent);
criterion_main!(benches);
