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
//! Two subjects are relocated throughout:
//!
//! * A bare `Arc<Payload, PerCore>`, which isolates the cost of a single
//!   relocation.
//! * A five-layer object tree, which is what actually crosses affinities in
//!   practice. Relocating it walks the whole graph and locks one slot table per
//!   layer, so it shows what the lock policy costs per message rather than per
//!   call.
//!
//! The suite covers these shapes:
//!
//! * `hit_path` / `miss_path` — uncontended cost of the two branches.
//! * `storm` — every thread relocates at once after a barrier release, which is
//!   what a fanout across all cores looks like.
//!
//! Paired with `thread_aware_relocate_cg.rs`, which covers `hit_path` and
//! `miss_path` under instruction-count measurement. The `storm` subgroup has no
//! Callgrind counterpart because it measures lock contention across threads,
//! which the single-threaded simulator cannot model.
//!
//! Run with: `cargo bench -p thread_aware --bench thread_aware_relocate`

#![allow(missing_docs, reason = "benchmark code")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{sync, thread};

use criterion::{BatchSize, Criterion, SamplingMode, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use thread_aware::affinity::{Affinity, pinned_affinities};
use thread_aware::{Arc, PerCore, ThreadAware, Unaware};

/// How far the oversubscribed case of the `storm` group exceeds the processor count.
///
/// Oversubscription is the point of that shape: it puts runnable threads in the
/// scheduler queue behind the ones holding a processor, which is the regime a
/// thread-per-core runtime reaches whenever it has more runnable work than cores,
/// and the regime in which a thread preempted while holding an exclusive lock
/// stalls everyone behind it.
///
/// The factor is small deliberately. Releasing a barrier costs roughly a
/// millisecond per few threads, and that cost lands inside the measured round; by
/// a few hundred threads it dwarfs the relocation work by orders of magnitude and
/// the shape measures thread wake-up rather than relocation. Two-times
/// oversubscription reaches the queued-behind regime while the barrier release
/// stays a minority of the round.
const STORM_OVERSUBSCRIPTION: usize = 2;

/// Source of distinct per-affinity identities.
static NEXT_VALUE_ID: AtomicU64 = AtomicU64::new(0);

/// Stand-in for the per-affinity state a consumer keeps behind an `Arc<T, PerCore>`,
/// such as a connection pipeline or a cache shard.
///
/// Relocation never inspects the payload. The identity exists so the setup can
/// verify that a relocation really swapped in the destination affinity's value,
/// and so the measured loop has something observable to consume.
#[derive(Debug)]
struct Payload {
    id: u64,
}

impl Payload {
    fn new() -> Self {
        Self {
            id: NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

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

// =========================================================================
// The relocated object tree.
// =========================================================================

/// Depth of the object tree, and therefore the number of distinct slot tables a
/// single tree relocation locks.
///
/// Relocation is a graph walk, so what a caller pays is set by the number of
/// thread-aware nodes reachable from the message, not by the cost of one call.
/// Five layers is a deliberately modest stand-in for a real message: a request
/// carrying a session, which holds a connection pool, which holds a resolver,
/// which holds a metrics sink.
const TREE_DEPTH: usize = 5;

/// Per-affinity state held behind one `Arc<_, PerCore>` node of the tree.
#[derive(Debug)]
struct Leaf {
    id: u64,
}

impl Leaf {
    fn new() -> Self {
        Self {
            id: NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// One layer of [`Tree`].
///
/// The field mix is the point of the type. `id` and `name` are thread-aware with
/// a no-op relocation, `flags` opts out entirely, and `shared` is a genuine
/// per-affinity node whose relocation takes a lock. Every layer owns a separate
/// slot table, so relocating the tree walks plain data and acquires exactly one
/// lock per layer, which is how relocation cost actually accrues in a consumer.
#[derive(Debug, Clone, ThreadAware)]
struct Layer {
    id: u64,
    name: &'static str,
    flags: Unaware<u32>,
    shared: Arc<Leaf, PerCore>,
    child: Option<Box<Self>>,
}

/// A message-shaped object tree of [`TREE_DEPTH`] layers.
///
/// This is the subject the multithreaded groups relocate, because a runtime
/// relocates whole messages rather than individual values.
#[derive(Debug, Clone, ThreadAware)]
struct Tree {
    root: Box<Layer>,
}

impl Tree {
    fn new() -> Self {
        let mut layer = None;

        for depth in 0..TREE_DEPTH {
            layer = Some(Box::new(Layer {
                id: depth as u64,
                name: "layer",
                flags: Unaware(0),
                shared: Arc::<Leaf, PerCore>::new(Leaf::new),
                child: layer,
            }));
        }

        Self {
            root: layer.expect("the loop runs at least once because TREE_DEPTH is nonzero"),
        }
    }

    /// Number of `Arc<_, PerCore>` nodes a relocation of this tree has to visit.
    fn node_count(&self) -> usize {
        self.leaf_ids().len()
    }

    /// Identity of every `Arc<_, PerCore>` node, in layer order.
    fn leaf_ids(&self) -> Vec<u64> {
        let mut ids = Vec::with_capacity(TREE_DEPTH);
        let mut layer = Some(&self.root);

        while let Some(current) = layer {
            ids.push(current.shared.id);
            layer = current.child.as_ref();
        }

        ids
    }
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
// =========================================================================

fn bench_miss_path(c: &mut Criterion) {
    let affinities = pinned_affinities(&[2]);
    let destination = affinities[1];

    let mut group = c.benchmark_group("thread_aware_relocate/miss_path");

    group.bench_function("new_affinity", |b| {
        b.iter_batched(
            || Arc::<Payload, PerCore>::new(Payload::new),
            |mut arc| {
                arc.relocate(black_box(None), black_box(destination));
                arc
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("tree", |b| {
        b.iter_batched(
            Tree::new,
            |mut tree| {
                tree.relocate(black_box(None), black_box(destination));
                tree
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// =========================================================================
// storm — every thread relocates the same subject at once.
// =========================================================================

/// Persistent worker pool that relocates one shared subject from every thread
/// simultaneously.
///
/// Threads are created once and reused for every Criterion sample. Spawning
/// hundreds of threads per sample would cost orders of magnitude more than the
/// operation under test and would drown out the signal entirely.
///
/// A round is driven by barriers rather than by any real-time wait: the
/// controller releases `start`, every worker performs exactly the requested
/// number of relocations, and `end` releases once they are all done.
///
/// Workers bracket their timed region with untimed load on both sides: they
/// relocate until every worker is awake before starting the clock, and again
/// after stopping it until every worker has stopped. Without that the shape
/// silently stops measuring what it claims. A Criterion sample gives each worker
/// far less work than a scheduler timeslice, so releasing the barrier is not
/// instantaneous relative to the work: the workers woken first would otherwise
/// time a machine that has not yet reached the advertised thread count, and on an
/// oversubscribed machine the first batch would run to completion and park before
/// the rest were ever scheduled. Each worker would then time a machine running at
/// the processor count rather than at the thread count, and the oversubscribed
/// shape would merely be a noisier copy of the saturated one. `run` asserts that
/// the windows really did overlap, so the flaw cannot return unnoticed.
///
/// Each worker times its own loop and the round reports the median of those
/// durations. A mean would let one stalled worker move the whole sample, and the
/// median still reflects what a participating thread pays per relocation.
struct RelocationStorm {
    start: sync::Arc<Barrier>,
    end: sync::Arc<Barrier>,
    iterations: sync::Arc<AtomicU64>,
    awake: sync::Arc<AtomicUsize>,
    finished: sync::Arc<AtomicUsize>,
    windows: sync::Arc<Mutex<Vec<(Instant, Instant)>>>,
    shutdown: sync::Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    thread_count: usize,
}

impl RelocationStorm {
    /// Creates a storm of `thread_count` workers, each relocating its own clone of
    /// the subject that `make` materializes across all participating affinities.
    fn new<T>(thread_count: usize, make: impl FnOnce(&[Affinity]) -> T) -> Self
    where
        T: ThreadAware + Clone + Send + 'static,
    {
        // At least two affinities even for the single-threaded shape, so that every
        // shape performs a cross-affinity relocation. A worker whose source equals
        // its destination would be measuring a different operation.
        let affinity_count = thread_count.max(2);
        let affinities = pinned_affinities(&[affinity_count]);
        let subject = make(&affinities);

        let start = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let end = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let iterations = sync::Arc::new(AtomicU64::new(0));
        let awake = sync::Arc::new(AtomicUsize::new(0));
        let finished = sync::Arc::new(AtomicUsize::new(0));
        let windows = sync::Arc::new(Mutex::new(Vec::with_capacity(thread_count)));
        let shutdown = sync::Arc::new(AtomicBool::new(false));

        let workers = (0..thread_count)
            .map(|index| {
                let destination = affinities[index];

                // Neighbouring affinity as the source, so the arguments differ per worker
                // the way they do when work fans out from one core to all the others.
                let source = affinities[index.wrapping_add(1) % affinity_count];
                let mut subject = subject.clone();

                let start = sync::Arc::clone(&start);
                let end = sync::Arc::clone(&end);
                let iterations = sync::Arc::clone(&iterations);
                let awake = sync::Arc::clone(&awake);
                let finished = sync::Arc::clone(&finished);
                let windows = sync::Arc::clone(&windows);
                let shutdown = sync::Arc::clone(&shutdown);

                thread::spawn(move || {
                    loop {
                        start.wait();

                        if shutdown.load(Ordering::Acquire) {
                            break;
                        }

                        let rounds = iterations.load(Ordering::Acquire);

                        // Lead-in load: releasing a barrier is not instantaneous, so
                        // without this the workers woken first would time part of a
                        // round during which the machine has not yet reached the
                        // thread count the shape advertises. Contend untimed until
                        // every worker is awake.
                        _ = awake.fetch_add(1, Ordering::AcqRel);

                        while awake.load(Ordering::Acquire) < thread_count {
                            subject.relocate(Some(source), destination);
                            black_box(&subject);
                        }

                        let started = Instant::now();

                        for _ in 0..rounds {
                            subject.relocate(Some(source), destination);
                            black_box(&subject);
                        }

                        let stopped = Instant::now();

                        // Tail load: keep contending until every worker has closed its
                        // window, so nobody measures a machine that has gone quiet.
                        _ = finished.fetch_add(1, Ordering::AcqRel);

                        while finished.load(Ordering::Acquire) < thread_count {
                            subject.relocate(Some(source), destination);
                            black_box(&subject);
                        }

                        windows.lock().unwrap_or_else(PoisonError::into_inner).push((started, stopped));

                        end.wait();
                    }
                })
            })
            .collect();

        Self {
            start,
            end,
            iterations,
            awake,
            finished,
            windows,
            shutdown,
            workers,
            thread_count,
        }
    }

    /// Borrows the recorded windows, tolerating a mutex poisoned by a worker panic.
    ///
    /// The worker's own panic message is the useful diagnostic in that case, so
    /// this must not mask it with a poisoning panic from the controller.
    fn lock_windows(&self) -> MutexGuard<'_, Vec<(Instant, Instant)>> {
        self.windows.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs one round in which every worker performs `iterations` relocations.
    ///
    /// The returned duration is the median over workers, so dividing by the
    /// iteration count yields the per-relocation cost under full contention.
    fn run(&self, iterations: u64) -> Duration {
        self.iterations.store(iterations, Ordering::Release);
        self.awake.store(0, Ordering::Release);
        self.finished.store(0, Ordering::Release);
        self.lock_windows().clear();

        self.start.wait();
        self.end.wait();

        let windows = self.lock_windows();

        let first_start = windows.iter().map(|&(started, _)| started).min();
        let last_start = windows.iter().map(|&(started, _)| started).max();
        let last_stop = windows.iter().map(|&(_, stopped)| stopped).max();

        let span = last_stop
            .zip(first_start)
            .map(|(stop, start)| stop.duration_since(start))
            .unwrap_or_default();

        // What the lead-in load failed to absorb: how far apart the workers still
        // entered their timed regions.
        let start_spread = last_start
            .zip(first_start)
            .map(|(last, first)| last.duration_since(first))
            .unwrap_or_default();

        let mut durations = windows
            .iter()
            .map(|&(started, stopped)| stopped.duration_since(started))
            .collect::<Vec<_>>();

        durations.sort_unstable();

        let Some(&median) = durations.get(durations.len() / 2) else {
            // Only reachable if a worker never recorded a window, which means it
            // panicked. Its own panic message is the useful diagnostic, so do not
            // bury it under an indexing panic from the controller.
            return Duration::ZERO;
        };

        // How many workers held an open timing window at once, averaged over the
        // round. The lead-in and tail loads bracket the timed region with full
        // load, so on a healthy round every window covers nearly the whole span
        // and this approaches the thread count.
        //
        // Half the advertised threads is the floor. Reaching it requires every
        // window to overlap every other one for at least half the round, which a
        // shape that has degenerated into a sequence of solo runs cannot do at any
        // thread count: contiguous windows sum to exactly one span no matter how
        // many workers there are.
        //
        // The check applies only once the timed work dominates the spread in start
        // times that the lead-in leaves behind, because a round shorter than that
        // spread cannot overlap however the code under test behaves. Expressing
        // the precondition as a ratio between two quantities the round measures
        // itself keeps it independent of the machine and of how Criterion sized
        // the round. In practice this separates cleanly: the rounds Criterion
        // measures clear the ratio by more than an order of magnitude, and only
        // its first ramp-up rounds fall short.
        let busy = durations.iter().sum::<Duration>().as_nanos();
        let expected = u128::try_from(self.thread_count).expect("a thread count always fits in u128");
        let aligned = median > start_spread.saturating_mul(10);

        assert!(
            self.thread_count == 1 || !aligned || busy.saturating_mul(2) >= span.as_nanos().saturating_mul(expected),
            "workers did not overlap: {busy} ns of open timing windows spread over a {} ns round \
             is far fewer than {expected} concurrent workers, so this sample measures a less \
             contended machine than the shape claims",
            span.as_nanos()
        );

        median
    }
}

impl Drop for RelocationStorm {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.start.wait();

        for worker in self.workers.drain(..) {
            worker.join().unwrap();
        }
    }
}

fn bench_storm(c: &mut Criterion) {
    let mut group = c.benchmark_group("thread_aware_relocate/storm");

    // Contention is a property of the round, not of its length, so the cost per
    // relocation is not linear in the iteration count that Criterion's default
    // sampling assumes. Flat sampling holds the shape of every sample constant.
    group.sampling_mode(SamplingMode::Flat);

    let saturated = SystemHardware::current().processors().len();

    // Three points, because the two failure modes have to be told apart. One
    // thread gives the uncontended cost. One thread per processor gives the
    // contention the lock actually has to survive. The oversubscribed case adds
    // the queue of threads waiting for a processor, which is what a machine
    // running more tasks than it has cores looks like.
    //
    // All of them relocate an object tree. A bare `Arc` was measured here too and
    // dropped: one acquisition per message collides so rarely that its run-to-run
    // spread exceeded any difference between locking policies, so it reported
    // noise. The `tree_` prefix is kept so the subject stays legible in stored
    // baselines and in benchmark output.
    let shapes = [
        ("threads_1", 1_usize),
        ("threads_saturated", saturated),
        ("threads_oversubscribed", saturated * STORM_OVERSUBSCRIPTION),
    ];

    for (name, thread_count) in shapes {
        let storm = RelocationStorm::new(thread_count, materialized_tree);

        group.bench_function(format!("tree_{name}"), |b| {
            b.iter_custom(|iterations| storm.run(iterations));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_hit_path, bench_miss_path, bench_storm);
criterion_main!(benches);
