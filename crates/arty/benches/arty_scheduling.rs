// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalent task-scheduling workloads for Arty and Tokio, measured with metabench.
//!
//! For allocation data, use `--criterion --allocations`. Threaded workloads do
//! not measure isolated instruction costs. Each case creates its runtime,
//! reusable vectors, and external waking thread before measurement. It reuses
//! this input for every measured iteration.
//! Both runtimes use the same external join driver. Worker-anywhere samples
//! submit from a task on a multi-thread runtime. Worker-local samples use an
//! Arty worker and a Tokio current-thread runtime with a `LocalSet`.
//!
//! Both runtimes use the same code outside the runtime to wait for task results.
//! For from-task workloads, timing starts inside the initial async task.
//! Allocation measurements also include that task and its setup.
//!
//! The `timeout` workload runs as many concurrent tasks as there are workers.
//! Each task starts
//! and cancels `count` request timeouts per iteration. This models deadlines
//! that almost never expire. It measures the cost of registering timers when all
//! workers use them at the same time. Tokio chooses each task's worker, so it does not
//! guarantee one task per worker.

#![expect(
    unreachable_pub,
    reason = "Triggered by metabench expansion for benchmark-local Gungraun configuration."
)]

use std::future::poll_fn;
use std::hint::black_box;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context as PollContext, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
use arty::task::{Context, JoinHandle};
use criterion::{BenchmarkId, Criterion, Throughput};
use gungraun::LibraryBenchmarkConfig;
#[cfg(target_os = "linux")]
use gungraun::client_requests::callgrind;
#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, EntryPoint};
use metabench::benchmark;
use thread_aware::Unaware;
use tokio::task::{JoinHandle as TokioJoinHandle, LocalSet};

const BLOCKING_THREADS: usize = 4;
const COUNTS: [usize; 2] = [1, 20];
const MAX_COUNT: usize = 20;
const WORKERS: [usize; 2] = [1, 4];

// Neither deadline expires during a benchmark. The background timer remains
// the earliest deadline, as in a server that always has a shorter timer armed.
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

#[derive(Clone, Copy, Debug)]
enum Workload {
    Spawn,
    Yield,
    RemoteWake,
    Timer,
    Timeout,
    WorkerAnywhere,
    WorkerLocal,
    Blocking,
}

impl Workload {
    const ALL: [Self; 6] = [
        Self::Spawn,
        Self::Yield,
        Self::RemoteWake,
        Self::Timer,
        Self::Timeout,
        Self::Blocking,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Yield => "yield",
            Self::RemoteWake => "wake",
            Self::Timer => "timer",
            Self::Timeout => "timeout",
            Self::WorkerAnywhere => "worker_anywhere",
            Self::WorkerLocal => "worker_local",
            Self::Blocking => "blocking",
        }
    }
}

#[derive(Debug)]
struct ArtyCase {
    runtime: Runtime,
    handles: Vec<JoinHandle<()>>,
    peer: WakePeer,
    workers: usize,
    count: usize,
    workload: Workload,
}

impl ArtyCase {
    fn new(workers: usize, count: usize, workload: Workload) -> Self {
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(workers))
            .blocking_pool(BlockingPoolPolicy::shared().max(BLOCKING_THREADS))
            .build()
            .expect("benchmark requires the selected number of available processors");
        let mut case = Self {
            runtime,
            handles: Vec::with_capacity(count.max(workers)),
            peer: WakePeer::new(),
            workers,
            count,
            workload,
        };
        // Cover every round-robin worker before measuring a single task.
        for _ in 0..workers {
            black_box(case.run(1));
        }
        case
    }

    fn remote<F>(&mut self, iterations: u64, factory: fn(Context, ()) -> F) -> Duration
    where
        F: Future<Output = ()> + 'static,
    {
        let scheduler = self.runtime.scheduler();
        let start = Instant::now();
        for _ in 0..iterations {
            self.handles.clear();
            self.handles.extend((0..self.count).map(|_| scheduler.spawn_anywhere((), factory)));
            for handle in &mut self.handles {
                futures::executor::block_on(black_box(handle)).expect("benchmark tasks finish before shutdown");
            }
        }
        start.elapsed()
    }

    fn run(&mut self, iterations: u64) -> Duration {
        let count = self.count;
        match self.workload {
            Workload::Spawn => self.remote(iterations, |_, ()| async { black_box(()) }),
            Workload::Yield => self.remote(iterations, |_, ()| async { YieldOnce::default().await }),
            Workload::Timer => self.remote(iterations, |cx, ()| async move { cx.clock().delay(Duration::from_millis(1)).await }),
            Workload::Timeout => {
                let operations = iterations
                    .checked_mul(u64::try_from(count).expect("benchmark counts fit in u64"))
                    .expect("requested benchmark operation count must fit in u64");
                let start = Instant::now();
                self.handles.clear();
                // Round-robin submissions place one task on each worker.
                self.handles.extend((0..self.workers).map(|_| {
                    self.runtime.scheduler().spawn_anywhere(operations, |cx, operations| async move {
                        let clock = cx.clock().clone();
                        timeout_churn(operations, |timeout| clock.delay(timeout)).await;
                    })
                }));
                for handle in &mut self.handles {
                    futures::executor::block_on(black_box(handle)).expect("benchmark tasks finish before shutdown");
                }
                start.elapsed()
            }
            Workload::RemoteWake => {
                let sender = self.peer.sender();
                let scheduler = self.runtime.scheduler();
                let start = Instant::now();
                for _ in 0..iterations {
                    self.handles.clear();
                    self.handles.extend((0..self.count).map(|_| {
                        // This workload intentionally shares one wake channel across workers.
                        scheduler.spawn_anywhere(Unaware(sender.clone()), |_, Unaware(sender)| async move {
                            RemoteWake::new(sender.clone()).await;
                        })
                    }));
                    for handle in &mut self.handles {
                        futures::executor::block_on(black_box(handle)).expect("benchmark tasks finish before shutdown");
                    }
                }
                start.elapsed()
            }
            Workload::WorkerAnywhere => self.run_worker_anywhere(iterations, false),
            Workload::WorkerLocal => self.run_worker_local(iterations, false),
            Workload::Blocking => {
                let start = Instant::now();
                for _ in 0..iterations {
                    self.handles.clear();
                    self.handles
                        .extend((0..count).map(|_| self.runtime.scheduler().spawn_blocking(|| black_box(()))));
                    for handle in &mut self.handles {
                        futures::executor::block_on(black_box(handle)).expect("benchmark tasks finish before shutdown");
                    }
                }
                start.elapsed()
            }
        }
    }

    fn run_worker_anywhere(&self, iterations: u64, instrument: bool) -> Duration {
        let count = self.count;
        futures::executor::block_on(self.runtime.scheduler().spawn_anywhere(
            (iterations, count, instrument),
            |cx, (iterations, count, instrument)| async move {
                if instrument {
                    toggle_callgrind_collection();
                }
                let elapsed = {
                    let mut handles = std::array::from_fn::<_, MAX_COUNT, _>(|_| None);
                    let start = Instant::now();
                    for _ in 0..iterations {
                        for handle in handles.iter_mut().take(count) {
                            *handle = Some(cx.scheduler().spawn_anywhere((), |()| async { black_box(()) }));
                        }
                        for handle in handles.iter_mut().take(count) {
                            black_box(handle.take().expect("all benchmark task slots are filled"))
                                .await
                                .expect("benchmark tasks finish before shutdown");
                        }
                    }
                    start.elapsed()
                };
                if instrument {
                    toggle_callgrind_collection();
                }
                elapsed
            },
        ))
        .expect("benchmark parent finishes before shutdown")
    }

    fn run_worker_local(&self, iterations: u64, instrument: bool) -> Duration {
        let count = self.count;
        futures::executor::block_on(self.runtime.scheduler().spawn_anywhere(
            (iterations, count, instrument),
            |cx, (iterations, count, instrument)| async move {
                if instrument {
                    toggle_callgrind_collection();
                }
                let elapsed = {
                    let mut handles = std::array::from_fn::<_, MAX_COUNT, _>(|_| None);
                    let start = Instant::now();
                    for _ in 0..iterations {
                        for handle in handles.iter_mut().take(count) {
                            *handle = Some(cx.scheduler().spawn(async |_| black_box(())));
                        }
                        for handle in handles.iter_mut().take(count) {
                            black_box(handle.take().expect("all benchmark task slots are filled"))
                                .await
                                .expect("benchmark tasks finish before shutdown");
                        }
                    }
                    start.elapsed()
                };
                if instrument {
                    toggle_callgrind_collection();
                }
                elapsed
            },
        ))
        .expect("benchmark parent finishes before shutdown")
    }
}

#[derive(Debug)]
struct TokioCase {
    runtime: tokio::runtime::Runtime,
    handles: Vec<TokioJoinHandle<()>>,
    peer: WakePeer,
    workers: usize,
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
            handles: Vec::with_capacity(count.max(workers)),
            peer: WakePeer::new(),
            workers,
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
            Workload::Timeout => {
                let operations = iterations
                    .checked_mul(u64::try_from(count).expect("benchmark counts fit in u64"))
                    .expect("requested benchmark operation count must fit in u64");
                let start = Instant::now();
                self.handles.clear();
                self.handles.extend((0..self.workers).map(|_| {
                    self.runtime
                        .spawn(async move { timeout_churn(operations, tokio::time::sleep).await })
                }));
                for handle in &mut self.handles {
                    futures::executor::block_on(black_box(handle)).expect("benchmark tasks do not panic");
                }
                start.elapsed()
            }
            Workload::RemoteWake => {
                let sender = self.peer.sender();
                self.remote(iterations, async move || RemoteWake::new(sender.clone()).await)
            }
            Workload::WorkerAnywhere => self.run_worker_anywhere(iterations, false),
            Workload::WorkerLocal => unreachable!("worker-local Tokio uses TokioLocalCase"),
            Workload::Blocking => {
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

    fn run_worker_anywhere(&self, iterations: u64, instrument: bool) -> Duration {
        let count = self.count;
        futures::executor::block_on(self.runtime.spawn(async move {
            if instrument {
                toggle_callgrind_collection();
            }
            let mut handles = std::array::from_fn::<_, MAX_COUNT, _>(|_| None);
            let start = Instant::now();
            for _ in 0..iterations {
                for handle in handles.iter_mut().take(count) {
                    *handle = Some(tokio::spawn(async { black_box(()) }));
                }
                for handle in handles.iter_mut().take(count) {
                    black_box(handle.take().expect("all benchmark task slots are filled"))
                        .await
                        .expect("benchmark tasks do not panic");
                }
            }
            let elapsed = start.elapsed();
            if instrument {
                toggle_callgrind_collection();
            }
            elapsed
        }))
        .expect("benchmark parent task does not panic")
    }
}

struct TokioLocalCase {
    runtime: tokio::runtime::Runtime,
    local: LocalSet,
    count: usize,
}

impl TokioLocalCase {
    fn new(count: usize) -> Self {
        let mut case = Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("benchmark runtime initialization must succeed"),
            local: LocalSet::new(),
            count,
        };
        black_box(case.run(1));
        case
    }

    fn run(&mut self, iterations: u64) -> Duration {
        self.run_with_instrumentation(iterations, false)
    }

    fn run_with_instrumentation(&mut self, iterations: u64, instrument: bool) -> Duration {
        let count = self.count;
        self.local.block_on(&self.runtime, async move {
            if instrument {
                toggle_callgrind_collection();
            }
            let mut handles = std::array::from_fn::<_, MAX_COUNT, _>(|_| None);
            let start = Instant::now();
            for _ in 0..iterations {
                for handle in handles.iter_mut().take(count) {
                    *handle = Some(tokio::task::spawn_local(async { black_box(()) }));
                }
                for handle in handles.iter_mut().take(count) {
                    black_box(handle.take().expect("all benchmark task slots are filled"))
                        .await
                        .expect("benchmark tasks do not panic");
                }
            }
            let elapsed = start.elapsed();
            if instrument {
                toggle_callgrind_collection();
            }
            elapsed
        })
    }
}

fn toggle_callgrind_collection() {
    #[cfg(target_os = "linux")]
    callgrind::toggle_collect();
}

#[cfg(target_os = "linux")]
fn worker_thread_config() -> LibraryBenchmarkConfig {
    let mut callgrind = Callgrind::default();
    callgrind
        .args(["--branch-sim=yes", "--instr-atstart=no", "--collect-atstart=no"])
        .entry_point(EntryPoint::None);
    let mut config = LibraryBenchmarkConfig::default();
    config.tool(callgrind.format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]));
    config
}

#[cfg(not(target_os = "linux"))]
fn worker_thread_config() -> LibraryBenchmarkConfig {
    LibraryBenchmarkConfig::default()
}

/// Arms and cancels `operations` request timeouts on the current worker while a background
/// timer stays armed. Each timeout is polled once, which registers it, then dropped, which
/// unregisters it.
async fn timeout_churn<T, F>(operations: u64, mut timer: F)
where
    T: Future<Output = ()>,
    F: FnMut(Duration) -> T,
{
    // Stack-pin to keep allocation measurements about timer registration, not boxing.
    let mut background = pin!(timer(BACKGROUND_TIMEOUT));
    arm(background.as_mut()).await;
    for _ in 0..operations {
        arm(pin!(timer(REQUEST_TIMEOUT))).await;
    }
}

async fn arm<T: Future<Output = ()>>(mut timer: Pin<&mut T>) {
    poll_fn(|cx| {
        assert!(
            timer.as_mut().poll(cx).is_pending(),
            "benchmark timeouts are far longer than any benchmark run"
        );
        Poll::Ready(())
    })
    .await;
}

// Use one self-wake without either runtime's convenience API.
#[derive(Debug, Default)]
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<()> {
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

    fn poll(mut self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<()> {
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
                let elements = if matches!(workload, Workload::Timeout) {
                    count.checked_mul(workers).expect("benchmark counts fit in usize")
                } else {
                    count
                };
                group.throughput(Throughput::Elements(u64::try_from(elements).expect("benchmark counts fit in u64")));
                group.bench_function(BenchmarkId::new(ARTY.benchmark_name(), &name), |bencher| {
                    let mut state = ArtyCase::new(workers, count, workload);
                    bencher.iter_custom(|iterations| arty_workload(&mut state, iterations).0);
                });
                group.bench_function(BenchmarkId::new(TOKIO.benchmark_name(), &name), |bencher| {
                    let mut state = TokioCase::new(workers, count, workload);
                    bencher.iter_custom(|iterations| tokio_workload(&mut state, iterations).0);
                });
            }
        }
    }
    group.finish();

    let mut group = criterion.benchmark_group(ARTY_ANYWHERE.group_name());
    for workers in WORKERS {
        for count in COUNTS {
            let name = format!("spawn_w{workers}_n{count}");
            group.throughput(Throughput::Elements(u64::try_from(count).expect("benchmark counts fit in u64")));
            group.bench_function(BenchmarkId::new(ARTY_ANYWHERE.benchmark_name(), &name), |bencher| {
                let state = ArtyCase::new(workers, count, Workload::WorkerAnywhere);
                bencher.iter_custom(|iterations| state.run_worker_anywhere(iterations, false));
            });
            group.bench_function(BenchmarkId::new(TOKIO_ANYWHERE.benchmark_name(), &name), |bencher| {
                let state = TokioCase::new(workers, count, Workload::WorkerAnywhere);
                bencher.iter_custom(|iterations| state.run_worker_anywhere(iterations, false));
            });
        }
    }
    group.finish();

    let mut group = criterion.benchmark_group(ARTY_LOCAL.group_name());
    for count in COUNTS {
        let name = format!("spawn_w1_n{count}");
        group.throughput(Throughput::Elements(u64::try_from(count).expect("benchmark counts fit in u64")));
        group.bench_function(BenchmarkId::new(ARTY_LOCAL.benchmark_name(), &name), |bencher| {
            let mut state = ArtyCase::new(1, count, Workload::WorkerLocal);
            bencher.iter_custom(|iterations| arty_local_workload(&mut state, iterations).0);
        });
        group.bench_function(BenchmarkId::new(TOKIO_LOCAL.benchmark_name(), &name), |bencher| {
            let mut state = TokioLocalCase::new(count);
            bencher.iter_custom(|iterations| tokio_local_workload(&mut state, iterations).0);
        });
    }
    group.finish();
}

#[benchmark(ARTY, "arty_scheduling/tasks", "Arty")]
#[bench::spawn_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::Spawn)), 1)]
#[bench::spawn_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::Spawn)), 1)]
#[bench::spawn_w4_n1(Box::new(ArtyCase::new(4, 1, Workload::Spawn)), 1)]
#[bench::spawn_w4_n20(Box::new(ArtyCase::new(4, 20, Workload::Spawn)), 1)]
#[bench::yield_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::Yield)), 1)]
#[bench::yield_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::Yield)), 1)]
#[bench::yield_w4_n1(Box::new(ArtyCase::new(4, 1, Workload::Yield)), 1)]
#[bench::yield_w4_n20(Box::new(ArtyCase::new(4, 20, Workload::Yield)), 1)]
#[bench::wake_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::RemoteWake)), 1)]
#[bench::wake_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::RemoteWake)), 1)]
#[bench::wake_w4_n1(Box::new(ArtyCase::new(4, 1, Workload::RemoteWake)), 1)]
#[bench::wake_w4_n20(Box::new(ArtyCase::new(4, 20, Workload::RemoteWake)), 1)]
#[bench::timer_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::Timer)), 1)]
#[bench::timer_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::Timer)), 1)]
#[bench::timer_w4_n1(Box::new(ArtyCase::new(4, 1, Workload::Timer)), 1)]
#[bench::timer_w4_n20(Box::new(ArtyCase::new(4, 20, Workload::Timer)), 1)]
#[bench::timeout_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::Timeout)), 1)]
#[bench::timeout_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::Timeout)), 1)]
#[bench::timeout_w4_n1(Box::new(ArtyCase::new(4, 1, Workload::Timeout)), 1)]
#[bench::timeout_w4_n20(Box::new(ArtyCase::new(4, 20, Workload::Timeout)), 1)]
#[bench::blocking_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::Blocking)), 1)]
#[bench::blocking_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::Blocking)), 1)]
#[bench::blocking_w4_n1(Box::new(ArtyCase::new(4, 1, Workload::Blocking)), 1)]
#[bench::blocking_w4_n20(Box::new(ArtyCase::new(4, 20, Workload::Blocking)), 1)]
fn arty_workload<S: std::borrow::BorrowMut<ArtyCase>>(mut owned_input: S, iterations: u64) -> (Duration, S) {
    let result = {
        let state = owned_input.borrow_mut();
        state.run(iterations)
    };
    (result, owned_input)
}

#[benchmark(TOKIO, "arty_scheduling/tasks", "Tokio")]
#[bench::spawn_w1_n1(Box::new(TokioCase::new(1, 1, Workload::Spawn)), 1)]
#[bench::spawn_w1_n20(Box::new(TokioCase::new(1, 20, Workload::Spawn)), 1)]
#[bench::spawn_w4_n1(Box::new(TokioCase::new(4, 1, Workload::Spawn)), 1)]
#[bench::spawn_w4_n20(Box::new(TokioCase::new(4, 20, Workload::Spawn)), 1)]
#[bench::yield_w1_n1(Box::new(TokioCase::new(1, 1, Workload::Yield)), 1)]
#[bench::yield_w1_n20(Box::new(TokioCase::new(1, 20, Workload::Yield)), 1)]
#[bench::yield_w4_n1(Box::new(TokioCase::new(4, 1, Workload::Yield)), 1)]
#[bench::yield_w4_n20(Box::new(TokioCase::new(4, 20, Workload::Yield)), 1)]
#[bench::wake_w1_n1(Box::new(TokioCase::new(1, 1, Workload::RemoteWake)), 1)]
#[bench::wake_w1_n20(Box::new(TokioCase::new(1, 20, Workload::RemoteWake)), 1)]
#[bench::wake_w4_n1(Box::new(TokioCase::new(4, 1, Workload::RemoteWake)), 1)]
#[bench::wake_w4_n20(Box::new(TokioCase::new(4, 20, Workload::RemoteWake)), 1)]
#[bench::timer_w1_n1(Box::new(TokioCase::new(1, 1, Workload::Timer)), 1)]
#[bench::timer_w1_n20(Box::new(TokioCase::new(1, 20, Workload::Timer)), 1)]
#[bench::timer_w4_n1(Box::new(TokioCase::new(4, 1, Workload::Timer)), 1)]
#[bench::timer_w4_n20(Box::new(TokioCase::new(4, 20, Workload::Timer)), 1)]
#[bench::timeout_w1_n1(Box::new(TokioCase::new(1, 1, Workload::Timeout)), 1)]
#[bench::timeout_w1_n20(Box::new(TokioCase::new(1, 20, Workload::Timeout)), 1)]
#[bench::timeout_w4_n1(Box::new(TokioCase::new(4, 1, Workload::Timeout)), 1)]
#[bench::timeout_w4_n20(Box::new(TokioCase::new(4, 20, Workload::Timeout)), 1)]
#[bench::blocking_w1_n1(Box::new(TokioCase::new(1, 1, Workload::Blocking)), 1)]
#[bench::blocking_w1_n20(Box::new(TokioCase::new(1, 20, Workload::Blocking)), 1)]
#[bench::blocking_w4_n1(Box::new(TokioCase::new(4, 1, Workload::Blocking)), 1)]
#[bench::blocking_w4_n20(Box::new(TokioCase::new(4, 20, Workload::Blocking)), 1)]
fn tokio_workload<S: std::borrow::BorrowMut<TokioCase>>(mut owned_input: S, iterations: u64) -> (Duration, S) {
    let result = {
        let state = owned_input.borrow_mut();
        state.run(iterations)
    };
    (result, owned_input)
}

#[benchmark(
    ARTY_ANYWHERE,
    "arty_scheduling/worker_anywhere_spawn",
    "Arty",
    gungraun_config = worker_thread_config()
)]
#[bench::spawn_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::WorkerAnywhere)), 1)]
#[bench::spawn_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::WorkerAnywhere)), 1)]
fn arty_anywhere_workload<S: std::borrow::Borrow<ArtyCase>>(owned_input: S, iterations: u64) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input.borrow().run_worker_anywhere(iterations, true);
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[benchmark(
    TOKIO_ANYWHERE,
    "arty_scheduling/worker_anywhere_spawn",
    "Tokio",
    gungraun_config = worker_thread_config()
)]
#[bench::spawn_w1_n1(Box::new(TokioCase::new(1, 1, Workload::WorkerAnywhere)), 1)]
#[bench::spawn_w1_n20(Box::new(TokioCase::new(1, 20, Workload::WorkerAnywhere)), 1)]
fn tokio_anywhere_workload<S: std::borrow::Borrow<TokioCase>>(owned_input: S, iterations: u64) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input.borrow().run_worker_anywhere(iterations, true);
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[benchmark(
    ARTY_LOCAL,
    "arty_scheduling/worker_local_spawn",
    "Arty",
    gungraun_config = worker_thread_config()
)]
#[bench::spawn_w1_n1(Box::new(ArtyCase::new(1, 1, Workload::WorkerLocal)), 1)]
#[bench::spawn_w1_n20(Box::new(ArtyCase::new(1, 20, Workload::WorkerLocal)), 1)]
fn arty_local_workload<S: std::borrow::BorrowMut<ArtyCase>>(mut owned_input: S, iterations: u64) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input.borrow_mut().run_worker_local(iterations, true);
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[benchmark(
    TOKIO_LOCAL,
    "arty_scheduling/worker_local_spawn",
    "Tokio",
    gungraun_config = worker_thread_config()
)]
#[bench::spawn_w1_n1(Box::new(TokioLocalCase::new(1)), 1)]
#[bench::spawn_w1_n20(Box::new(TokioLocalCase::new(20)), 1)]
fn tokio_local_workload<S: std::borrow::BorrowMut<TokioLocalCase>>(
    mut owned_input: S,
    iterations: u64,
) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input.borrow_mut().run_with_instrumentation(iterations, true);
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[cfg(target_os = "linux")]
metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    gungraun = {
        config = LibraryBenchmarkConfig::default().tool(
            Callgrind::default()
                .args(["--branch-sim=yes"])
                .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
        );
    },
    benchmarks = [ARTY, TOKIO, ARTY_ANYWHERE, TOKIO_ANYWHERE, ARTY_LOCAL, TOKIO_LOCAL],
);

#[cfg(not(target_os = "linux"))]
metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [ARTY, TOKIO, ARTY_ANYWHERE, TOKIO_ANYWHERE, ARTY_LOCAL, TOKIO_LOCAL],
);
