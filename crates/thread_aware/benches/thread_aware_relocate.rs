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
//! The suite covers three shapes:
//!
//! * `hit_path` / `miss_path` — uncontended cost of the two branches.
//! * `storm` — every thread relocates at once after a barrier release, which is
//!   what a fanout across all cores looks like.
//! * `handoff` — two threads exchange messages and relocate each one on the
//!   receiving thread, which is what a request pipeline looks like. The channel
//!   dominates this shape, so each variant is paired with a `_transport`
//!   control that omits the relocation.
//!
//! Paired with `thread_aware_relocate_cg.rs`, which covers `hit_path` and
//! `miss_path` under instruction-count measurement. The multithreaded groups
//! have no Callgrind counterpart because the simulator is single-threaded.
//!
//! Run with: `cargo bench -p thread_aware --bench thread_aware_relocate`

#![allow(missing_docs, reason = "benchmark code")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]

use std::hint::black_box;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Barrier, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{sync, thread};

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, Criterion, SamplingMode, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};
use thread_aware::affinity::{Affinity, pinned_affinities};
use thread_aware::{Arc, PerCore, ThreadAware};

/// Number of threads in the high case of the `storm` group.
///
/// Chosen to oversubscribe every machine we benchmark on, so the measurement
/// reflects a lock that many more threads want than the hardware can run at
/// once. That is the regime the hit path has to survive: a thread-per-core
/// runtime multiplies each spawn by however many tasks are already queued.
const STORM_THREADS: usize = 250;

/// Source of distinct [`Payload`] identities.
static NEXT_PAYLOAD_ID: AtomicU64 = AtomicU64::new(0);

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
            id: NEXT_PAYLOAD_ID.fetch_add(1, Ordering::Relaxed),
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

    group.finish();
}

// =========================================================================
// storm — every thread relocates the same value at once.
// =========================================================================

/// Persistent worker pool that relocates one shared `Arc<Payload, PerCore>` from
/// every thread simultaneously.
///
/// Threads are created once and reused for every Criterion sample. Spawning
/// hundreds of threads per sample would cost orders of magnitude more than the
/// operation under test and would drown out the signal entirely.
///
/// A round is driven by a pair of barriers rather than by any real-time wait:
/// the controller releases `start`, every worker performs exactly the requested
/// number of relocations, and `end` releases once they are all done.
///
/// Each worker times its own loop and the round reports the median of those
/// durations. Timing the round from the controller would instead measure how
/// long the operating system took to wake several hundred threads, and the mean
/// over workers would let one stalled worker move the whole sample. The median
/// is robust to both while still reflecting what a participating thread pays
/// per relocation.
struct RelocationStorm {
    start: sync::Arc<Barrier>,
    end: sync::Arc<Barrier>,
    iterations: sync::Arc<AtomicU64>,
    elapsed_nanos: sync::Arc<Mutex<Vec<u64>>>,
    shutdown: sync::Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl RelocationStorm {
    fn new(thread_count: usize) -> Self {
        let affinities = pinned_affinities(&[thread_count]);
        let arc = materialized(&affinities);

        let start = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let end = sync::Arc::new(Barrier::new(thread_count.saturating_add(1)));
        let iterations = sync::Arc::new(AtomicU64::new(0));
        let elapsed_nanos = sync::Arc::new(Mutex::new(Vec::with_capacity(thread_count)));
        let shutdown = sync::Arc::new(AtomicBool::new(false));

        let workers = affinities
            .iter()
            .enumerate()
            .map(|(index, &destination)| {
                // Neighbouring affinity as the source, so the arguments differ per worker
                // the way they do when work fans out from one core to all the others.
                let source = affinities[index.wrapping_add(1) % thread_count];
                let mut subject = arc.clone();

                let start = sync::Arc::clone(&start);
                let end = sync::Arc::clone(&end);
                let iterations = sync::Arc::clone(&iterations);
                let elapsed_nanos = sync::Arc::clone(&elapsed_nanos);
                let shutdown = sync::Arc::clone(&shutdown);

                thread::spawn(move || {
                    loop {
                        start.wait();

                        if shutdown.load(Ordering::Acquire) {
                            break;
                        }

                        let rounds = iterations.load(Ordering::Acquire);
                        let started = Instant::now();

                        for _ in 0..rounds {
                            subject.relocate(Some(source), destination);
                            black_box(subject.id);
                        }

                        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                        elapsed_nanos.lock().unwrap().push(elapsed);

                        end.wait();
                    }
                })
            })
            .collect();

        Self {
            start,
            end,
            iterations,
            elapsed_nanos,
            shutdown,
            workers,
        }
    }

    /// Runs one round in which every worker performs `iterations` relocations.
    ///
    /// The returned duration is the median over workers, so dividing by the
    /// iteration count yields the per-relocation cost under full contention.
    fn run(&self, iterations: u64) -> Duration {
        self.iterations.store(iterations, Ordering::Release);
        self.elapsed_nanos.lock().unwrap().clear();

        self.start.wait();
        self.end.wait();

        let mut samples = self.elapsed_nanos.lock().unwrap();
        samples.sort_unstable();

        Duration::from_nanos(samples[samples.len() / 2])
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
    let shapes = [
        ("threads_1", 1_usize),
        ("threads_saturated", saturated),
        ("threads_250", STORM_THREADS),
    ];

    for (name, thread_count) in shapes {
        let storm = RelocationStorm::new(thread_count);

        group.bench_function(name, |b| {
            b.iter_custom(|iterations| storm.run(iterations));
        });
    }

    group.finish();
}

// =========================================================================
// handoff — threads exchange messages and relocate them on arrival.
// =========================================================================

/// Message queue owned by one participant of the `handoff` benchmark.
///
/// The receiver sits behind a mutex because `std::sync::mpsc::Receiver` is not
/// `Sync`, and `par_bench` hands every closure a shared reference to the state
/// it captures. The lock is only ever taken by its owning thread, so it adds a
/// constant uncontended cost to every iteration and does not distort the
/// before/after comparison.
struct Mailbox {
    sender: Sender<Arc<Payload, PerCore>>,
    receiver: Mutex<Receiver<Arc<Payload, PerCore>>>,
}

impl Mailbox {
    fn new() -> Self {
        let (sender, receiver) = channel();

        Self {
            sender,
            receiver: Mutex::new(receiver),
        }
    }
}

/// Registers one handoff benchmark over `participant_count` threads.
///
/// Each participant sends one message per iteration to the next participant and,
/// when `relocate` is set, relocates the message it receives to its own affinity.
/// With a single participant the message is sent to itself, which isolates the
/// transport from any cross-thread traffic.
///
/// The transport dominates this shape: a channel round trip costs tens of times
/// what an uncontended relocation does. The `relocate = false` variant measures
/// that floor, so the relocation cost is the difference between the two variants
/// rather than a fraction of either.
fn bench_handoff_shape(
    pool: &mut ThreadPool,
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    participant_count: usize,
    relocate: bool,
) {
    let affinities = pinned_affinities(&[participant_count]);
    let template = materialized(&affinities);
    let mailboxes = (0..participant_count).map(|_| Mailbox::new()).collect::<Vec<_>>();
    let groups = NonZero::new(participant_count).expect("benchmark shapes always have at least one participant");

    _ = Run::new()
        .groups(groups)
        .iter(|args| {
            let own = args.meta().group_index();
            let peer = own.wrapping_add(1) % participant_count;

            mailboxes[peer].sender.send(template.clone()).unwrap();

            let mut message = mailboxes[own].receiver.lock().unwrap().recv().unwrap();

            if relocate {
                message.relocate(Some(affinities[peer]), affinities[own]);
            }

            black_box(message.id);

            // Returned as cleanup state so the drop lands outside the measured region.
            message
        })
        .execute_criterion_on(pool, group, name);
}

fn bench_handoff(c: &mut Criterion) {
    let hardware = SystemHardware::current();

    let Some(two_processors) = hardware.processors().to_builder().take(nz!(2)) else {
        // A single-processor machine cannot express the two-thread shape.
        return;
    };
    let one_processor = hardware
        .processors()
        .to_builder()
        .take(nz!(1))
        .expect("every machine has at least one processor");

    let mut single = ThreadPool::new(&one_processor);
    let mut pair = ThreadPool::new(&two_processors);

    let mut group = c.benchmark_group("thread_aware_relocate/handoff");

    bench_handoff_shape(&mut single, &mut group, "threads_1", 1, true);
    bench_handoff_shape(&mut single, &mut group, "threads_1_transport", 1, false);
    bench_handoff_shape(&mut pair, &mut group, "threads_2", 2, true);
    bench_handoff_shape(&mut pair, &mut group, "threads_2_transport", 2, false);

    group.finish();
}

criterion_group!(benches, bench_hit_path, bench_miss_path, bench_storm, bench_handoff);
criterion_main!(benches);
