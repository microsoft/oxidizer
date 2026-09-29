// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Matched Arty/Tokio scheduling workloads, measured with metabench.
//!
//! Use `--criterion --allocations`. Threaded workloads are not isolated instruction costs.
//! Runtimes, reusable vectors, and the external waking thread are prepared before measurement.
//! Both runtimes use the same external join driver. From-task/local wall-clock samples measure
//! inside the async entry; allocation measurement also includes that entry and its setup.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use arty::rt::config::{ProcessorCount, WorkerPoolPolicy};
use arty::rt::{Builtins, JoinHandle, Runtime, TaskScheduler};
use criterion::{BenchmarkId, Criterion, Throughput};
use metabench::benchmark;
use tokio::task::{JoinHandle as TokioJoinHandle, LocalSet};

const BLOCKING_THREADS: usize = 4;
const COUNTS: [usize; 2] = [1, 100];
const WORKERS: [usize; 2] = [1, 4];

#[derive(Clone, Copy, Debug)]
enum Workload {
    Spawn,
    Yield,
    RemoteWake,
    Timer,
    FromTask,
    Local,
    System,
}

impl Workload {
    const ALL: [Self; 7] = [
        Self::Spawn,
        Self::Yield,
        Self::RemoteWake,
        Self::Timer,
        Self::FromTask,
        Self::Local,
        Self::System,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Yield => "yield",
            Self::RemoteWake => "wake",
            Self::Timer => "timer",
            Self::FromTask => "nested",
            Self::Local => "local",
            Self::System => "system",
        }
    }
}

#[derive(Debug)]
struct ArtyCase {
    _runtime: Runtime,
    scheduler: TaskScheduler,
    handles: Vec<JoinHandle<()>>,
    peer: WakePeer,
    count: usize,
    workload: Workload,
}

impl ArtyCase {
    fn new(workers: usize, count: usize, workload: Workload) -> Self {
        let runtime = Runtime::builder()
            .processor_count(ProcessorCount::exactly(
                NonZeroUsize::new(workers).expect("benchmark worker counts are nonzero"),
            ))
            .worker_pool_policy(WorkerPoolPolicy::shared(BLOCKING_THREADS))
            .build()
            .expect("benchmark requires the selected number of available processors");
        let scheduler = runtime.task_scheduler();
        let mut case = Self {
            _runtime: runtime,
            scheduler,
            handles: Vec::with_capacity(count),
            peer: WakePeer::new(),
            count,
            workload,
        };
        // Cover every round-robin worker before measuring a single task.
        for _ in 0..workers {
            black_box(case.run(1));
        }
        case
    }

    fn remote<FF, F>(&mut self, iterations: u64, factory: FF) -> Duration
    where
        FF: FnOnce(Builtins) -> F + Clone + Send + 'static,
        F: Future<Output = ()> + 'static,
    {
        let start = Instant::now();
        for _ in 0..iterations {
            self.handles.clear();
            self.handles.extend((0..self.count).map(|_| self.scheduler.spawn(factory.clone())));
            for handle in &mut self.handles {
                futures::executor::block_on(black_box(handle));
            }
        }
        start.elapsed()
    }

    fn run(&mut self, iterations: u64) -> Duration {
        let count = self.count;
        match self.workload {
            Workload::Spawn => self.remote(iterations, async |_| black_box(())),
            Workload::Yield => self.remote(iterations, async |_| YieldOnce::default().await),
            Workload::Timer => self.remote(iterations, async |cx| cx.clock().delay(Duration::from_millis(1)).await),
            Workload::RemoteWake => {
                let sender = self.peer.sender();
                self.remote(iterations, async move |_| RemoteWake::new(sender.clone()).await)
            }
            Workload::FromTask => self
                .scheduler
                .spawn(async move |cx| {
                    let mut handles = Vec::with_capacity(count);
                    let start = Instant::now();
                    for _ in 0..iterations {
                        handles.clear();
                        handles.extend((0..count).map(|_| cx.scheduler().spawn(async |_| black_box(()))));
                        for handle in &mut handles {
                            black_box(handle).await;
                        }
                    }
                    start.elapsed()
                })
                .wait(),
            Workload::Local => self
                .scheduler
                .spawn(async move |cx| {
                    let scheduler = cx.local_scheduler().expect("local benchmark runs on its associated worker");
                    let mut handles = Vec::with_capacity(count);
                    let start = Instant::now();
                    for _ in 0..iterations {
                        handles.clear();
                        handles.extend((0..count).map(|_| scheduler.spawn(async || black_box(()))));
                        for handle in &mut handles {
                            black_box(handle).await;
                        }
                    }
                    start.elapsed()
                })
                .wait(),
            Workload::System => {
                let start = Instant::now();
                for _ in 0..iterations {
                    self.handles.clear();
                    self.handles
                        .extend((0..count).map(|_| self.scheduler.spawn_system(|| black_box(()))));
                    for handle in &mut self.handles {
                        futures::executor::block_on(black_box(handle));
                    }
                }
                start.elapsed()
            }
        }
    }
}

#[derive(Debug)]
struct TokioCase {
    runtime: tokio::runtime::Runtime,
    handles: Vec<TokioJoinHandle<()>>,
    peer: WakePeer,
    count: usize,
    workload: Workload,
}

impl TokioCase {
    fn new(workers: usize, count: usize, workload: Workload) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .max_blocking_threads(BLOCKING_THREADS)
            .enable_time()
            .build()
            .expect("benchmark runtime initialization must succeed");
        let mut case = Self {
            runtime,
            handles: Vec::with_capacity(count),
            peer: WakePeer::new(),
            count,
            workload,
        };
        // Match the Arty preparation count; Tokio still chooses its own task placement.
        for _ in 0..workers {
            black_box(case.run(1));
        }
        case
    }

    fn remote<FF, F>(&mut self, iterations: u64, factory: FF) -> Duration
    where
        FF: FnOnce() -> F + Clone,
        F: Future<Output = ()> + Send + 'static,
    {
        let start = Instant::now();
        for _ in 0..iterations {
            self.handles.clear();
            self.handles.extend((0..self.count).map(|_| self.runtime.spawn(factory.clone()())));
            for handle in &mut self.handles {
                futures::executor::block_on(black_box(handle)).expect("benchmark tasks do not panic");
            }
        }
        start.elapsed()
    }

    fn run(&mut self, iterations: u64) -> Duration {
        let count = self.count;
        match self.workload {
            Workload::Spawn => self.remote(iterations, async || black_box(())),
            Workload::Yield => self.remote(iterations, async || YieldOnce::default().await),
            Workload::Timer => self.remote(iterations, async || tokio::time::sleep(Duration::from_millis(1)).await),
            Workload::RemoteWake => {
                let sender = self.peer.sender();
                self.remote(iterations, async move || RemoteWake::new(sender.clone()).await)
            }
            Workload::FromTask => futures::executor::block_on(self.runtime.spawn(async move {
                let mut handles = Vec::with_capacity(count);
                let start = Instant::now();
                for _ in 0..iterations {
                    handles.clear();
                    handles.extend((0..count).map(|_| tokio::spawn(async { black_box(()) })));
                    for handle in &mut handles {
                        black_box(handle).await.expect("benchmark tasks do not panic");
                    }
                }
                start.elapsed()
            }))
            .expect("benchmark parent task does not panic"),
            Workload::Local => self.runtime.block_on(LocalSet::new().run_until(async {
                let mut handles = Vec::with_capacity(count);
                let start = Instant::now();
                for _ in 0..iterations {
                    handles.clear();
                    handles.extend((0..count).map(|_| tokio::task::spawn_local(async { black_box(()) })));
                    for handle in &mut handles {
                        black_box(handle).await.expect("benchmark tasks do not panic");
                    }
                }
                start.elapsed()
            })),
            Workload::System => {
                let start = Instant::now();
                for _ in 0..iterations {
                    self.handles.clear();
                    self.handles
                        .extend((0..count).map(|_| self.runtime.spawn_blocking(|| black_box(()))));
                    for handle in &mut self.handles {
                        futures::executor::block_on(black_box(handle)).expect("benchmark tasks do not panic");
                    }
                }
                start.elapsed()
            }
        }
    }
}

// A single self-wake, independent of either runtime's convenience APIs.
#[derive(Debug, Default)]
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[derive(Debug)]
struct WakeRequest {
    ready: Arc<AtomicBool>,
    waker: Waker,
}

#[derive(Debug)]
struct WakePeer {
    sender: Option<mpsc::Sender<WakeRequest>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl WakePeer {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<WakeRequest>();
        let worker = thread::spawn(move || {
            for request in receiver {
                request.ready.store(true, Ordering::Release);
                request.waker.wake();
            }
        });
        Self {
            sender: Some(sender),
            worker: Some(worker),
        }
    }

    fn sender(&self) -> mpsc::Sender<WakeRequest> {
        self.sender.as_ref().expect("sender is present until peer destruction").clone()
    }
}

impl Drop for WakePeer {
    fn drop(&mut self) {
        drop(self.sender.take());
        self.worker
            .take()
            .expect("peer is joined exactly once")
            .join()
            .expect("wake peer does not panic");
    }
}

#[derive(Debug)]
struct RemoteWake {
    sender: Option<mpsc::Sender<WakeRequest>>,
    ready: Arc<AtomicBool>,
}

impl RemoteWake {
    fn new(sender: mpsc::Sender<WakeRequest>) -> Self {
        Self {
            sender: Some(sender),
            ready: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Future for RemoteWake {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.ready.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            if let Some(sender) = self.sender.take() {
                sender
                    .send(WakeRequest {
                        ready: Arc::clone(&self.ready),
                        waker: cx.waker().clone(),
                    })
                    .expect("wake peer lives until every measured task completes");
            }
            Poll::Pending
        }
    }
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ARTY.group_name());
    for workload in Workload::ALL {
        for workers in WORKERS {
            for count in COUNTS {
                let name = format!("{}_w{workers}_n{count}", workload.name());
                group.throughput(Throughput::Elements(u64::try_from(count).expect("benchmark counts fit in u64")));
                group.bench_function(BenchmarkId::new(ARTY.benchmark_name(), &name), |bencher| {
                    let mut state = ArtyCase::new(workers, count, workload);
                    bencher.iter_custom(|iterations| arty_workload(&mut state, iterations));
                });
                group.bench_function(BenchmarkId::new(TOKIO.benchmark_name(), &name), |bencher| {
                    let mut state = TokioCase::new(workers, count, workload);
                    bencher.iter_custom(|iterations| tokio_workload(&mut state, iterations));
                });
            }
        }
    }
    group.finish();
}

#[benchmark(ARTY, "arty_scheduling/tasks", "Arty")]
#[bench::spawn_w1_n1(&mut ArtyCase::new(1, 1, Workload::Spawn), 1)]
#[bench::spawn_w1_n100(&mut ArtyCase::new(1, 100, Workload::Spawn), 1)]
#[bench::spawn_w4_n1(&mut ArtyCase::new(4, 1, Workload::Spawn), 1)]
#[bench::spawn_w4_n100(&mut ArtyCase::new(4, 100, Workload::Spawn), 1)]
#[bench::yield_w1_n1(&mut ArtyCase::new(1, 1, Workload::Yield), 1)]
#[bench::yield_w1_n100(&mut ArtyCase::new(1, 100, Workload::Yield), 1)]
#[bench::yield_w4_n1(&mut ArtyCase::new(4, 1, Workload::Yield), 1)]
#[bench::yield_w4_n100(&mut ArtyCase::new(4, 100, Workload::Yield), 1)]
#[bench::wake_w1_n1(&mut ArtyCase::new(1, 1, Workload::RemoteWake), 1)]
#[bench::wake_w1_n100(&mut ArtyCase::new(1, 100, Workload::RemoteWake), 1)]
#[bench::wake_w4_n1(&mut ArtyCase::new(4, 1, Workload::RemoteWake), 1)]
#[bench::wake_w4_n100(&mut ArtyCase::new(4, 100, Workload::RemoteWake), 1)]
#[bench::timer_w1_n1(&mut ArtyCase::new(1, 1, Workload::Timer), 1)]
#[bench::timer_w1_n100(&mut ArtyCase::new(1, 100, Workload::Timer), 1)]
#[bench::timer_w4_n1(&mut ArtyCase::new(4, 1, Workload::Timer), 1)]
#[bench::timer_w4_n100(&mut ArtyCase::new(4, 100, Workload::Timer), 1)]
#[bench::nested_w1_n1(&mut ArtyCase::new(1, 1, Workload::FromTask), 1)]
#[bench::nested_w1_n100(&mut ArtyCase::new(1, 100, Workload::FromTask), 1)]
#[bench::nested_w4_n1(&mut ArtyCase::new(4, 1, Workload::FromTask), 1)]
#[bench::nested_w4_n100(&mut ArtyCase::new(4, 100, Workload::FromTask), 1)]
#[bench::local_w1_n1(&mut ArtyCase::new(1, 1, Workload::Local), 1)]
#[bench::local_w1_n100(&mut ArtyCase::new(1, 100, Workload::Local), 1)]
#[bench::local_w4_n1(&mut ArtyCase::new(4, 1, Workload::Local), 1)]
#[bench::local_w4_n100(&mut ArtyCase::new(4, 100, Workload::Local), 1)]
#[bench::system_w1_n1(&mut ArtyCase::new(1, 1, Workload::System), 1)]
#[bench::system_w1_n100(&mut ArtyCase::new(1, 100, Workload::System), 1)]
#[bench::system_w4_n1(&mut ArtyCase::new(4, 1, Workload::System), 1)]
#[bench::system_w4_n100(&mut ArtyCase::new(4, 100, Workload::System), 1)]
fn arty_workload(state: &mut ArtyCase, iterations: u64) -> Duration {
    state.run(iterations)
}

#[benchmark(TOKIO, "arty_scheduling/tasks", "Tokio")]
#[bench::spawn_w1_n1(&mut TokioCase::new(1, 1, Workload::Spawn), 1)]
#[bench::spawn_w1_n100(&mut TokioCase::new(1, 100, Workload::Spawn), 1)]
#[bench::spawn_w4_n1(&mut TokioCase::new(4, 1, Workload::Spawn), 1)]
#[bench::spawn_w4_n100(&mut TokioCase::new(4, 100, Workload::Spawn), 1)]
#[bench::yield_w1_n1(&mut TokioCase::new(1, 1, Workload::Yield), 1)]
#[bench::yield_w1_n100(&mut TokioCase::new(1, 100, Workload::Yield), 1)]
#[bench::yield_w4_n1(&mut TokioCase::new(4, 1, Workload::Yield), 1)]
#[bench::yield_w4_n100(&mut TokioCase::new(4, 100, Workload::Yield), 1)]
#[bench::wake_w1_n1(&mut TokioCase::new(1, 1, Workload::RemoteWake), 1)]
#[bench::wake_w1_n100(&mut TokioCase::new(1, 100, Workload::RemoteWake), 1)]
#[bench::wake_w4_n1(&mut TokioCase::new(4, 1, Workload::RemoteWake), 1)]
#[bench::wake_w4_n100(&mut TokioCase::new(4, 100, Workload::RemoteWake), 1)]
#[bench::timer_w1_n1(&mut TokioCase::new(1, 1, Workload::Timer), 1)]
#[bench::timer_w1_n100(&mut TokioCase::new(1, 100, Workload::Timer), 1)]
#[bench::timer_w4_n1(&mut TokioCase::new(4, 1, Workload::Timer), 1)]
#[bench::timer_w4_n100(&mut TokioCase::new(4, 100, Workload::Timer), 1)]
#[bench::nested_w1_n1(&mut TokioCase::new(1, 1, Workload::FromTask), 1)]
#[bench::nested_w1_n100(&mut TokioCase::new(1, 100, Workload::FromTask), 1)]
#[bench::nested_w4_n1(&mut TokioCase::new(4, 1, Workload::FromTask), 1)]
#[bench::nested_w4_n100(&mut TokioCase::new(4, 100, Workload::FromTask), 1)]
#[bench::local_w1_n1(&mut TokioCase::new(1, 1, Workload::Local), 1)]
#[bench::local_w1_n100(&mut TokioCase::new(1, 100, Workload::Local), 1)]
#[bench::local_w4_n1(&mut TokioCase::new(4, 1, Workload::Local), 1)]
#[bench::local_w4_n100(&mut TokioCase::new(4, 100, Workload::Local), 1)]
#[bench::system_w1_n1(&mut TokioCase::new(1, 1, Workload::System), 1)]
#[bench::system_w1_n100(&mut TokioCase::new(1, 100, Workload::System), 1)]
#[bench::system_w4_n1(&mut TokioCase::new(4, 1, Workload::System), 1)]
#[bench::system_w4_n100(&mut TokioCase::new(4, 100, Workload::System), 1)]
fn tokio_workload(state: &mut TokioCase, iterations: u64) -> Duration {
    state.run(iterations)
}

metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [ARTY, TOKIO],
);
