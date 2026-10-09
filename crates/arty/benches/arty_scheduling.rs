// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Matched Arty/Tokio scheduling workloads, measured with metabench.
//!
//! Use `--criterion --allocations`. Threaded workloads are not isolated instruction costs.
//! Runtimes, reusable vectors, and the external waking thread are prepared before measurement.
//! Both runtimes use the same external join driver. From-task wall-clock
//! samples measure inside the async entry; allocation measurement also includes
//! that entry and its setup.
//!
//! The `timeout` workload runs one task per worker, concurrently. Each task arms and cancels
//! `count` request timeouts per iteration, the common pattern of a deadline that almost never
//! fires. It measures how timer registration scales when every worker uses timers at once.
//! Tokio places its tasks itself, so one task per Tokio worker is likely but not guaranteed.

use std::future::poll_fn;
use std::hint::black_box;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
use arty::task::{Builtins, JoinHandle};
use criterion::{BenchmarkId, Criterion, Throughput};
#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
use metabench::benchmark;
use thread_aware::Unaware;
use tokio::task::JoinHandle as TokioJoinHandle;

const BLOCKING_THREADS: usize = 4;
const COUNTS: [usize; 2] = [1, 20];
const WORKERS: [usize; 2] = [1, 4];

// Neither deadline is reached during a benchmark. The background timer keeps every request
// timeout from being the earliest deadline, as in a server that always has a shorter timer armed.
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

#[derive(Clone, Copy, Debug)]
enum Workload {
    Spawn,
    Yield,
    RemoteWake,
    Timer,
    Timeout,
    FromTask,
    Blocking,
}

impl Workload {
    const ALL: [Self; 7] = [
        Self::Spawn,
        Self::Yield,
        Self::RemoteWake,
        Self::Timer,
        Self::Timeout,
        Self::FromTask,
        Self::Blocking,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Yield => "yield",
            Self::RemoteWake => "wake",
            Self::Timer => "timer",
            Self::Timeout => "timeout",
            Self::FromTask => "nested",
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

    fn remote<F>(&mut self, iterations: u64, factory: fn(Builtins, ()) -> F) -> Duration
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
            Workload::FromTask => futures::executor::block_on(self.runtime.scheduler().spawn_anywhere(
                (iterations, count),
                |cx, (iterations, count)| async move {
                    let mut handles = Vec::with_capacity(count);
                    let start = Instant::now();
                    for _ in 0..iterations {
                        handles.clear();
                        handles.extend((0..count).map(|_| cx.scheduler().spawn(async |_| black_box(()))));
                        for handle in &mut handles {
                            black_box(handle).await.expect("benchmark tasks finish before shutdown");
                        }
                    }
                    start.elapsed()
                },
            ))
            .expect("benchmark parent finishes before shutdown"),
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
                let elements = if matches!(workload, Workload::Timeout) {
                    count.checked_mul(workers).expect("benchmark counts fit in usize")
                } else {
                    count
                };
                group.throughput(Throughput::Elements(u64::try_from(elements).expect("benchmark counts fit in u64")));
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
#[bench::spawn_w1_n20(&mut ArtyCase::new(1, 20, Workload::Spawn), 1)]
#[bench::spawn_w4_n1(&mut ArtyCase::new(4, 1, Workload::Spawn), 1)]
#[bench::spawn_w4_n20(&mut ArtyCase::new(4, 20, Workload::Spawn), 1)]
#[bench::yield_w1_n1(&mut ArtyCase::new(1, 1, Workload::Yield), 1)]
#[bench::yield_w1_n20(&mut ArtyCase::new(1, 20, Workload::Yield), 1)]
#[bench::yield_w4_n1(&mut ArtyCase::new(4, 1, Workload::Yield), 1)]
#[bench::yield_w4_n20(&mut ArtyCase::new(4, 20, Workload::Yield), 1)]
#[bench::wake_w1_n1(&mut ArtyCase::new(1, 1, Workload::RemoteWake), 1)]
#[bench::wake_w1_n20(&mut ArtyCase::new(1, 20, Workload::RemoteWake), 1)]
#[bench::wake_w4_n1(&mut ArtyCase::new(4, 1, Workload::RemoteWake), 1)]
#[bench::wake_w4_n20(&mut ArtyCase::new(4, 20, Workload::RemoteWake), 1)]
#[bench::timer_w1_n1(&mut ArtyCase::new(1, 1, Workload::Timer), 1)]
#[bench::timer_w1_n20(&mut ArtyCase::new(1, 20, Workload::Timer), 1)]
#[bench::timer_w4_n1(&mut ArtyCase::new(4, 1, Workload::Timer), 1)]
#[bench::timer_w4_n20(&mut ArtyCase::new(4, 20, Workload::Timer), 1)]
#[bench::timeout_w1_n1(&mut ArtyCase::new(1, 1, Workload::Timeout), 1)]
#[bench::timeout_w1_n20(&mut ArtyCase::new(1, 20, Workload::Timeout), 1)]
#[bench::timeout_w4_n1(&mut ArtyCase::new(4, 1, Workload::Timeout), 1)]
#[bench::timeout_w4_n20(&mut ArtyCase::new(4, 20, Workload::Timeout), 1)]
#[bench::nested_w1_n1(&mut ArtyCase::new(1, 1, Workload::FromTask), 1)]
#[bench::nested_w1_n20(&mut ArtyCase::new(1, 20, Workload::FromTask), 1)]
#[bench::nested_w4_n1(&mut ArtyCase::new(4, 1, Workload::FromTask), 1)]
#[bench::nested_w4_n20(&mut ArtyCase::new(4, 20, Workload::FromTask), 1)]
#[bench::blocking_w1_n1(&mut ArtyCase::new(1, 1, Workload::Blocking), 1)]
#[bench::blocking_w1_n20(&mut ArtyCase::new(1, 20, Workload::Blocking), 1)]
#[bench::blocking_w4_n1(&mut ArtyCase::new(4, 1, Workload::Blocking), 1)]
#[bench::blocking_w4_n20(&mut ArtyCase::new(4, 20, Workload::Blocking), 1)]
fn arty_workload(state: &mut ArtyCase, iterations: u64) -> Duration {
    state.run(iterations)
}

#[benchmark(TOKIO, "arty_scheduling/tasks", "Tokio")]
#[bench::spawn_w1_n1(&mut TokioCase::new(1, 1, Workload::Spawn), 1)]
#[bench::spawn_w1_n20(&mut TokioCase::new(1, 20, Workload::Spawn), 1)]
#[bench::spawn_w4_n1(&mut TokioCase::new(4, 1, Workload::Spawn), 1)]
#[bench::spawn_w4_n20(&mut TokioCase::new(4, 20, Workload::Spawn), 1)]
#[bench::yield_w1_n1(&mut TokioCase::new(1, 1, Workload::Yield), 1)]
#[bench::yield_w1_n20(&mut TokioCase::new(1, 20, Workload::Yield), 1)]
#[bench::yield_w4_n1(&mut TokioCase::new(4, 1, Workload::Yield), 1)]
#[bench::yield_w4_n20(&mut TokioCase::new(4, 20, Workload::Yield), 1)]
#[bench::wake_w1_n1(&mut TokioCase::new(1, 1, Workload::RemoteWake), 1)]
#[bench::wake_w1_n20(&mut TokioCase::new(1, 20, Workload::RemoteWake), 1)]
#[bench::wake_w4_n1(&mut TokioCase::new(4, 1, Workload::RemoteWake), 1)]
#[bench::wake_w4_n20(&mut TokioCase::new(4, 20, Workload::RemoteWake), 1)]
#[bench::timer_w1_n1(&mut TokioCase::new(1, 1, Workload::Timer), 1)]
#[bench::timer_w1_n20(&mut TokioCase::new(1, 20, Workload::Timer), 1)]
#[bench::timer_w4_n1(&mut TokioCase::new(4, 1, Workload::Timer), 1)]
#[bench::timer_w4_n20(&mut TokioCase::new(4, 20, Workload::Timer), 1)]
#[bench::timeout_w1_n1(&mut TokioCase::new(1, 1, Workload::Timeout), 1)]
#[bench::timeout_w1_n20(&mut TokioCase::new(1, 20, Workload::Timeout), 1)]
#[bench::timeout_w4_n1(&mut TokioCase::new(4, 1, Workload::Timeout), 1)]
#[bench::timeout_w4_n20(&mut TokioCase::new(4, 20, Workload::Timeout), 1)]
#[bench::nested_w1_n1(&mut TokioCase::new(1, 1, Workload::FromTask), 1)]
#[bench::nested_w1_n20(&mut TokioCase::new(1, 20, Workload::FromTask), 1)]
#[bench::nested_w4_n1(&mut TokioCase::new(4, 1, Workload::FromTask), 1)]
#[bench::nested_w4_n20(&mut TokioCase::new(4, 20, Workload::FromTask), 1)]
#[bench::blocking_w1_n1(&mut TokioCase::new(1, 1, Workload::Blocking), 1)]
#[bench::blocking_w1_n20(&mut TokioCase::new(1, 20, Workload::Blocking), 1)]
#[bench::blocking_w4_n1(&mut TokioCase::new(4, 1, Workload::Blocking), 1)]
#[bench::blocking_w4_n20(&mut TokioCase::new(4, 20, Workload::Blocking), 1)]
fn tokio_workload(state: &mut TokioCase, iterations: u64) -> Duration {
    state.run(iterations)
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
    benchmarks = [ARTY, TOKIO],
);

#[cfg(not(target_os = "linux"))]
metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [ARTY, TOKIO],
);
