// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock benchmarks for `thread_aware::Arc<T, S>::relocate`.
//!
//! `relocate` is the hot path of the crate: a thread-per-core runtime calls it
//! once per cross-core spawn, for every `Arc<_, PerCore>` reachable in the
//! relocated object graph. The steady state is that the destination affinity
//! already holds its value, so the call reduces to cloning a `sync::Arc` out of
//! a slot.
//!
//! The following subjects are relocated throughout:
//!
//! * A bare `Arc<Payload, PerCore>`, which isolates the cost of a single
//!   relocation.
//! * A five-layer object tree, which is what actually crosses affinities in
//!   practice. Relocating it walks the whole graph and locks one slot table per
//!   layer, so it reports what the lock policy costs per message rather than per
//!   call.
//!
//! The suite covers these shapes:
//!
//! * `hit_path` / `miss_path` — uncontended cost of the two branches.
//! * `concurrent` — the hit-path cost with as many concurrent workers as
//!   processors, and beyond, each relocating into its own slot, so there is no
//!   shared lock to serialize on.
//!
//! Paired with `thread_aware_relocate_cg.rs`, which covers `hit_path` and
//! `miss_path` under instruction-count measurement. The `concurrent` subgroup has
//! no Callgrind counterpart because it measures scaling across threads, which the
//! single-threaded simulator cannot model.
//!
//! Run with: `cargo bench -p thread_aware --bench thread_aware_relocate`

#![allow(clippy::unwrap_used, reason = "benchmark code")]

use std::hint::black_box;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{sync, thread};

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use thread_aware::affinity::{Affinity, pinned_affinities};
use thread_aware::{Arc, PerCore, ThreadAware};

#[path = "support/mod.rs"]
mod support;

use support::{Payload, TREE_DEPTH, Tree};

/// How far the oversubscribed case of the `concurrent` group exceeds the
/// processor count.
///
/// Oversubscription is the point of that shape: it puts more runnable workers in
/// the scheduler queue than there are processors to run them, the regime a
/// thread-per-core runtime reaches whenever it has more runnable work than cores.
/// Every worker still relocates into its own already-filled slot, so no two
/// workers ever share a lock; the shape exposes how the uncontended hit path
/// behaves under scheduler pressure, not lock contention.
///
/// A small multiple is enough to reach that regime. Higher factors only add more
/// scheduler queueing without exercising a different code path.
const CONCURRENT_OVERSUBSCRIPTION: usize = 2;

/// Builds an `Arc<Payload, PerCore>` whose slot is already materialized for every
/// affinity in `affinities`.
///
/// Every benchmark that measures the hit path needs this, because a slot is only
/// filled by a relocation that misses first.
fn materialized(affinities: &[Affinity]) -> Arc<Payload, PerCore> {
    let arc = Arc::<Payload, PerCore>::new(Payload::new);

    let mut ids = Vec::with_capacity(affinities.len());

    for &affinity in affinities {
        let mut probe = arc.clone();
        probe.relocate(None, affinity);
        ids.push(probe.id);
    }

    ids.sort_unstable();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), distinct, "every affinity must hold its own value");

    arc
}

/// Sizes the shared slot table(s) of `subject` and fills only `primer`'s slot.
///
/// Relocating a throwaway clone into a primer affinity allocates every slot table
/// the subject reaches and materializes the primer slot, while leaving every other
/// slot empty. Timing a relocation between two affinities distinct from `primer`
/// therefore measures a genuine cross-slot miss with the table already allocated,
/// keeping the one-time table allocation out of the measurement.
fn seed_slot_table<T: ThreadAware + Clone>(subject: &T, primer: Affinity) {
    let mut seed = subject.clone();
    seed.relocate(None, primer);
}

/// Builds a [`Tree`] whose every layer is already materialized for every affinity
/// in `affinities`.
fn materialized_tree(affinities: &[Affinity]) -> Tree {
    let tree = Tree::new();

    assert_eq!(tree.node_count(), TREE_DEPTH, "the tree must be as deep as it claims");

    let mut ids = Vec::with_capacity(affinities.len().saturating_mul(TREE_DEPTH));

    for &affinity in affinities {
        let mut probe = tree.clone();
        probe.relocate(None, affinity);
        ids.extend(probe.leaf_ids());
    }

    ids.sort_unstable();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), distinct, "every layer of every affinity must hold its own value");

    tree
}

// =========================================================================
// hit_path — destination affinity already holds a value.
// =========================================================================

fn bench_hit_path(c: &mut Criterion) {
    let affinities = pinned_affinities(&[2]);
    let (source, destination) = (affinities[0], affinities[1]);
    let arc = materialized(&affinities);

    let mut group = c.benchmark_group("thread_aware_relocate/hit_path");

    let mut subject = arc.clone();
    group.bench_function("pre_materialized", |b| {
        b.iter(|| {
            black_box(&mut subject).relocate(black_box(Some(source)), black_box(destination));
        });
    });

    let mut subject = arc;
    group.bench_function("same_affinity", |b| {
        b.iter(|| {
            black_box(&mut subject).relocate(black_box(Some(destination)), black_box(destination));
        });
    });

    let mut subject = materialized_tree(&affinities);
    group.bench_function("tree", |b| {
        b.iter(|| {
            black_box(&mut subject).relocate(black_box(Some(source)), black_box(destination));
        });
    });

    group.finish();
}

// =========================================================================
// miss_path — destination affinity is empty, so the value is materialized.
//             A primer affinity sizes the shared slot table before timing, so
//             the measurement is the cross-slot miss itself — the exclusive
//             re-probe, the factory call, and the two slot writes — rather than
//             the one-time table allocation.
// =========================================================================

fn bench_miss_path(c: &mut Criterion) {
    let affinities = pinned_affinities(&[3]);
    let (primer, source, destination) = (affinities[0], affinities[1], affinities[2]);

    let mut group = c.benchmark_group("thread_aware_relocate/miss_path");

    group.bench_function("new_affinity", |b| {
        b.iter_batched(
            || {
                let arc = Arc::<Payload, PerCore>::new(Payload::new);
                seed_slot_table(&arc, primer);
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
                seed_slot_table(&tree, primer);
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
//              slots at once, with no shared lock to serialize on.
// =========================================================================

/// Persistent worker pool that measures a relocation performed concurrently from
/// every worker, each into its own already-materialized destination slot.
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
/// destinations are distinct and already populated, no two workers share a slot
/// lock, so that figure reflects the hit path while every worker is busy.
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
    /// the subject that `make` materializes across all participating affinities.
    fn new<T>(thread_count: usize, make: impl FnOnce(&[Affinity]) -> T) -> Self
    where
        T: ThreadAware + Clone + Send + 'static,
    {
        // Force an affinity floor even for the single-worker case, so that every
        // shape performs a cross-affinity relocation. A worker whose source equals
        // its destination would be measuring a different operation.
        let affinity_count = thread_count.max(2);
        let affinities = pinned_affinities(&[affinity_count]);
        let subject = make(&affinities);

        let ready = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let start = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let end = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let batch = sync::Arc::new(AtomicU64::new(0));
        let shutdown = sync::Arc::new(AtomicBool::new(false));

        let workers = (0..thread_count)
            .map(|index| {
                let destination = affinities[index];

                // Neighbouring affinity as the source, so the arguments differ per worker
                // the way they do when work fans out from one core to all the others.
                let source = affinities[index.wrapping_add(1) % affinity_count];
                let mut subject = subject.clone();

                let ready = sync::Arc::clone(&ready);
                let start = sync::Arc::clone(&start);
                let end = sync::Arc::clone(&end);
                let batch = sync::Arc::clone(&batch);
                let shutdown = sync::Arc::clone(&shutdown);

                thread::spawn(move || {
                    loop {
                        ready.wait();
                        start.wait();

                        if shutdown.load(Ordering::Acquire) {
                            break;
                        }

                        for _ in 0..batch.load(Ordering::Acquire) {
                            subject.relocate(Some(source), destination);
                            black_box(&subject);
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
    // slot, so the measured relocations are all hits and no two workers share a
    // lock; the reported per-relocation time therefore reflects the hit path scaling
    // as the machine fills rather than lock contention. The oversubscribed shape
    // adds scheduler pressure, not lock contention. The uncontended single-worker
    // cost belongs to `hit_path`, so it is deliberately absent here: one worker would
    // only add this harness's thread-handoff overhead to the same number.
    //
    // The subject is the object tree, which is what actually crosses affinities in
    // a consumer. It locks one slot table per layer, so each message does several
    // slot acquisitions and the per-message lock cost is large enough to resolve; a
    // bare `Arc` does one acquisition, too little to resolve above the harness noise.
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
