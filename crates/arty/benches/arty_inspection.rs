// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cost of runtime inspection in task creation, polling, and blocking queues.
//!
//! Runtimes and the diagnostic caller are created outside measurement. Every
//! workload runs on persistent Arty threads, as in `arty_scheduling`. Each
//! iteration represents 64 operations. The polling case uses one long-lived
//! task per sample so task creation does not dominate repeated polls.
//! Each benchmark case reuses one runtime across warm-up and all samples.
//! Raw sample timing keeps one runtime entry around all operations and avoids
//! a timer on each operation.
//!
//! `collecting` requests a process snapshot, then waits 10 ms before the next
//! request. It includes collection but not a requested poll-measurement window.
//! `recording` enables all runtime events without backtraces. Neither mode
//! represents the cost of a disconnected, idle monitor.
//!
//! Run with `cargo bench -p arty --bench arty_inspection -- --noplot`.
//! The same source can run against the pre-integration runtime. Only `disabled`
//! and `idle` are meaningful baseline cases there.

use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "monitor")]
use std::sync::mpsc;
use std::task::{Context as PollContext, Poll};
use std::thread;
use std::time::{Duration, Instant};

use arty::runtime::{BlockingPoolPolicy, MonitorPolicy, Runtime, WorkersPolicy};
use arty::task::JoinHandle;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
#[cfg(feature = "monitor")]
use seismograph::recorder::{Configuration, Policy};
#[cfg(feature = "monitor")]
use seismograph::snapshot::SnapshotOptions;

const OPERATIONS: u64 = 64;

#[derive(Clone, Copy)]
enum Mode {
    Disabled,
    #[cfg(feature = "monitor")]
    Idle,
    #[cfg(feature = "monitor")]
    Collecting,
    #[cfg(feature = "monitor")]
    Recording,
}

impl Mode {
    #[cfg(feature = "monitor")]
    const ALL: &[Self] = &[Self::Disabled, Self::Idle, Self::Collecting, Self::Recording];
    #[cfg(not(feature = "monitor"))]
    const ALL: &[Self] = &[Self::Disabled];

    fn name(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            #[cfg(feature = "monitor")]
            Self::Idle => "idle",
            #[cfg(feature = "monitor")]
            Self::Collecting => "collecting",
            #[cfg(feature = "monitor")]
            Self::Recording => "recording",
        }
    }
}

#[derive(Clone, Copy)]
enum Workload {
    LocalSpawn,
    RepeatedPoll,
    RemoteBatch,
    BlockingBatch,
}

impl Workload {
    const ALL: [Self; 4] = [Self::LocalSpawn, Self::RepeatedPoll, Self::RemoteBatch, Self::BlockingBatch];

    fn name(self) -> &'static str {
        match self {
            Self::LocalSpawn => "local_spawn_w1",
            Self::RepeatedPoll => "repeated_poll_w1",
            Self::RemoteBatch => "remote_batch_w4",
            Self::BlockingBatch => "blocking_batch_w4",
        }
    }
}

struct Case {
    runtime: Runtime,
    handles: Vec<JoinHandle<()>>,
    collector: Option<thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    captures: Arc<AtomicU64>,
}

impl Case {
    fn new(mode: Mode, workload: Workload) -> Self {
        #[cfg(feature = "monitor")]
        seismograph::recorder(Configuration::default());
        let workers = if matches!(workload, Workload::RemoteBatch | Workload::BlockingBatch) {
            4
        } else {
            1
        };
        let policy = if matches!(mode, Mode::Disabled) {
            MonitorPolicy::disabled()
        } else {
            MonitorPolicy::enabled()
        };
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(workers))
            .blocking_pool(BlockingPoolPolicy::shared().max(4))
            .monitor(policy)
            .build()
            .expect("benchmark runtime configuration is valid");
        let stop = Arc::new(AtomicBool::new(false));
        let captures = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "monitor")]
        let collector = matches!(mode, Mode::Collecting).then(|| {
            let stop = Arc::clone(&stop);
            let captures = Arc::clone(&captures);
            let (ready, started) = mpsc::sync_channel(1);
            let collector = thread::spawn(move || {
                black_box(seismograph::snapshot(SnapshotOptions::default()).expect("benchmark collection succeeds"));
                captures.fetch_add(1, Ordering::Relaxed);
                ready.send(()).expect("benchmark caller waits for collection");
                while !stop.load(Ordering::Acquire) {
                    thread::park_timeout(Duration::from_millis(10));
                    black_box(seismograph::snapshot(SnapshotOptions::default()).expect("benchmark collection succeeds"));
                    captures.fetch_add(1, Ordering::Relaxed);
                }
            });
            started
                .recv_timeout(Duration::from_secs(5))
                .expect("benchmark collector must complete its first snapshot before measurement");
            collector
        });
        #[cfg(not(feature = "monitor"))]
        let collector = None;
        #[cfg(feature = "monitor")]
        if matches!(mode, Mode::Recording) {
            seismograph::recorder(Configuration {
                runtime_tasks: Policy::all(false),
                ..Configuration::default()
            });
        }
        let mut case = Self {
            runtime,
            handles: Vec::with_capacity(usize::try_from(OPERATIONS).expect("64 operations fit in usize")),
            collector,
            stop,
            captures,
        };
        black_box(case.run(workload, 4));
        case
    }

    fn run(&mut self, workload: Workload, iterations: u64) -> Duration {
        let start = Instant::now();
        match workload {
            Workload::LocalSpawn => {
                let task = self
                    .runtime
                    .scheduler()
                    .spawn_anywhere(iterations * OPERATIONS, |cx, operations| async move {
                        for _ in 0..operations {
                            cx.scheduler()
                                .spawn(async |_| black_box(()))
                                .await
                                .expect("benchmark child succeeds");
                        }
                    });
                futures::executor::block_on(task).expect("benchmark parent succeeds");
            }
            Workload::RepeatedPoll => {
                let task = self
                    .runtime
                    .scheduler()
                    .spawn_anywhere(iterations * OPERATIONS, |_, operations| RepeatedPoll(operations));
                futures::executor::block_on(task).expect("benchmark task succeeds");
            }
            Workload::RemoteBatch | Workload::BlockingBatch => {
                for _ in 0..iterations {
                    self.handles.clear();
                    for _ in 0..OPERATIONS {
                        let task = if matches!(workload, Workload::RemoteBatch) {
                            self.runtime.scheduler().spawn_anywhere((), |_, ()| async { black_box(()) })
                        } else {
                            self.runtime.scheduler().spawn_blocking(|| black_box(()))
                        };
                        self.handles.push(task);
                    }
                    for task in &mut self.handles {
                        futures::executor::block_on(black_box(task)).expect("benchmark task succeeds");
                    }
                }
            }
        }
        start.elapsed()
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(collector) = self.collector.take() {
            collector.thread().unpark();
            collector.join().expect("benchmark diagnostic caller succeeds");
            eprintln!("Completed process snapshots: {}", self.captures.load(Ordering::Relaxed));
        }
        #[cfg(feature = "monitor")]
        seismograph::recorder(Configuration::default());
    }
}

struct RepeatedPoll(u64);

impl Future for RepeatedPoll {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<()> {
        if self.0 == 0 {
            Poll::Ready(())
        } else {
            self.0 = black_box(self.0 - 1);
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

fn benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("arty_inspection/tasks");
    group.throughput(Throughput::Elements(OPERATIONS));
    for workload in Workload::ALL {
        for &mode in Mode::ALL {
            let mut case = None;
            group.bench_function(BenchmarkId::new(workload.name(), mode.name()), |bencher| {
                let case = case.get_or_insert_with(|| Case::new(mode, workload));
                bencher.iter_custom(|iterations| case.run(workload, iterations));
            });
        }
    }
    group.finish();
}

criterion_group! {
    name = inspection;
    config = Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = benchmarks
}
criterion_main!(inspection);
