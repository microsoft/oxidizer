// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arty and Tokio contention benchmarks for common nested-task patterns.
//!
//! Each direct-admission case submits ten outer tasks that each start ten inner operations.
//! The relocated-cache case uses `black_box` on each cache access.
//! This prevents the compiler from removing the operation that the benchmark measures.
//!
//! The request-fanout cases first submit one core task. That task uses
//! `spawn_anywhere` to select workers for ten request tasks. Each request uses
//! regular `spawn` calls for ten child operations on its selected worker.
//! This models a server that distributes requests but keeps each request's work local.
//! The Tokio case uses the same task tree and cache operations, but Tokio chooses
//! where its child tasks run. The direct-admission cases remain separate controls.
//!
//! The request-work cases keep that task tree and one cache access per child.
//! Each child then runs 16 or 256 dependent integer-mixing rounds.
//! Both runtimes use the same function and await all 100 children.
//! These bounded synthetic sizes expose how useful work changes the share of
//! runtime overhead. They do not model a specific server or replace the
//! original single-access request-fanout controls.
//!
//! The request-suspend cases keep that task tree and one cache access per child.
//! Each child registers a pending receive before it signals its request task.
//! The request waits for all ten signals, then releases all ten children.
//! Each child resumes, runs 16 dependent integer-mixing rounds, and returns its result.
//! Both runtimes use the same futures channels and await every child.
//! The channels and their allocations are part of the measured work.
//! This bounded synthetic workload measures suspension and wake overhead.
//! It does not model network I/O, timer delays, or production request sizes.
//!
//! Each benchmark case creates one runtime before measurement and reuses it for
//! every measured iteration. The relocated-cache cases also create and reuse
//! one cache.
//!
//! The cache is a mutex behind a per-thread `performables::arc::Arc`. Arty
//! updates that value's state for the worker before the outer task starts.
//! Tokio uses the same cache type without that update.
//!
//! A waker tells a worker that a task is ready to run again.
//! On its first poll, each nested retained-waker future saves a clone of its
//! task waker. It uses that clone to request another poll and returns `Pending`.
//! Its next poll returns `Ready`.
//!
//! Arty keeps each inner task on its outer task's worker. This worker also owns
//! the relocated cache for that workload. Tokio can move child tasks between
//! workers. The cases start and finish child tasks the same way.
//! They do the same cache work and use the same task count at one and eight workers.
//! These workloads do not represent
//! general runtime performance.
//!
//! Native cases enable Callgrind instrumentation across all threads only while
//! the workload runs. Runtime construction, warm-up, and destruction stay outside
//! measurement. The default function-entry collection toggle only measures the
//! caller thread and excludes the workers that runtime construction starts.
//! These cases replace that toggle with global instrumentation requests.
//! Each start resets the simulated caches, so cache metrics include that cold start.
//! Callgrind cannot model how tasks share workers or compete across workers.
//! Use Criterion to compare runtime performance for these workloads.

use std::future::poll_fn;
use std::hint::black_box;
use std::pin::Pin;
use std::task::{Context as PollContext, Poll, Waker};
use std::time::{Duration, Instant};

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Context;
use criterion::{BenchmarkId, Criterion, Throughput};
use futures::channel::oneshot;
#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, EntryPoint, LibraryBenchmarkConfig};
use metabench::benchmark;
use performables::arc::{Arc, PerThread};
use performables::sync::mutex::Mutex;
use thread_aware::{Thread, ThreadAware};

const INNER_OPERATIONS: usize = 10;
const CONCURRENCY: usize = 10;
const CACHE_ENTRIES: usize = 8;
const WORKERS: [usize; 2] = [1, 8];
const REQUEST_WORK: [usize; 2] = [16, 256];
const SUSPENDED_WORK: usize = 16;

#[derive(Debug, Default)]
struct CacheState {
    entries: [u64; CACHE_ENTRIES],
}

#[derive(Clone, Debug)]
struct Cache {
    inner: Arc<Mutex<CacheState>, PerThread>,
}

impl Cache {
    fn new() -> Self {
        Self {
            inner: Arc::<Mutex<CacheState>, PerThread>::new_with(new_cache_mutex),
        }
    }

    fn touch(&self, operation: usize) -> u64 {
        let value = {
            let mut state = self.inner.lock_result().expect("benchmark cache mutex is never poisoned");
            let index = operation % CACHE_ENTRIES;
            let value = state.entries[index].wrapping_add(u64::try_from(operation).expect("benchmark operation fits in u64") + 1);
            state.entries[index] = value;
            value
        };
        black_box(value)
    }
}

impl ThreadAware for Cache {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.inner.relocate(source, destination);
    }
}

fn new_cache_mutex() -> Mutex<CacheState> {
    Mutex::new(CacheState::default())
}

#[derive(Debug)]
struct ArtyCase {
    runtime: Runtime,
    cache: Cache,
}

impl ArtyCase {
    fn new(workers: usize) -> Self {
        let case = Self {
            runtime: Runtime::builder()
                .workers(WorkersPolicy::exactly(workers))
                .build()
                .expect("benchmark runtime construction must succeed"),
            cache: Cache::new(),
        };
        for _ in 0..workers {
            black_box(case.run(CONCURRENCY));
        }
        case
    }

    fn run(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        let start = Instant::now();
        let scheduler = self.runtime.scheduler();
        let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
        for handle in &mut handles {
            *handle = Some(scheduler.spawn_anywhere(self.cache.clone(), arty_outer));
        }
        futures::executor::block_on(async move {
            for handle in handles {
                let Some(handle) = handle else {
                    unreachable!("all benchmark outer task slots are filled");
                };
                handle.await.expect("benchmark outer task finishes before shutdown");
            }
        });
        start.elapsed()
    }

    fn run_requests(&self) -> Duration {
        let start = Instant::now();
        let core = self.runtime.scheduler().spawn_anywhere(self.cache.clone(), arty_core);
        futures::executor::block_on(core).expect("benchmark core task finishes before shutdown");
        start.elapsed()
    }

    fn run_request_work(&self, rounds: usize) -> Duration {
        let start = Instant::now();
        let core = self
            .runtime
            .scheduler()
            .spawn_anywhere((self.cache.clone(), rounds), arty_work_core);
        futures::executor::block_on(core).expect("benchmark core task finishes before shutdown");
        start.elapsed()
    }

    fn run_request_suspend(&self) -> Duration {
        let start = Instant::now();
        let core = self.runtime.scheduler().spawn_anywhere(self.cache.clone(), arty_suspended_core);
        futures::executor::block_on(core).expect("benchmark core task finishes before shutdown");
        start.elapsed()
    }
}

async fn arty_work_core(cx: Context, (cache, rounds): (Cache, usize)) {
    let scheduler = cx.scheduler();
    let handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| {
        scheduler.spawn_anywhere((cx.clone(), cache.clone(), rounds), |(cx, cache, rounds)| {
            arty_work_outer(cx, cache, rounds)
        })
    });
    for handle in handles {
        handle.await.expect("benchmark request task finishes before shutdown");
    }
}

async fn arty_work_outer(cx: Context, cache: Cache, rounds: usize) {
    let scheduler = cx.scheduler();
    let handles = std::array::from_fn::<_, INNER_OPERATIONS, _>(|operation| {
        let cache = cache.clone();
        scheduler.spawn(move |_| async move { request_work(&cache, operation, rounds) })
    });
    for handle in handles {
        black_box(handle.await.expect("benchmark inner task finishes before shutdown"));
    }
}

fn request_work(cache: &Cache, operation: usize, rounds: usize) -> u64 {
    mix_request_work(cache.touch(operation), rounds)
}

fn mix_request_work(mut value: u64, rounds: usize) -> u64 {
    for round in 0..black_box(rounds) {
        value = value
            .rotate_left(7)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(u64::try_from(round).expect("bounded benchmark round fits in u64"));
    }
    black_box(value)
}

async fn suspended_request_work(cache: Cache, operation: usize, suspended: oneshot::Sender<()>, mut resumed: oneshot::Receiver<()>) -> u64 {
    let value = cache.touch(operation);
    let mut suspended = Some(suspended);
    poll_fn(|cx| {
        let result = Pin::new(&mut resumed).poll(cx);
        if result.is_pending()
            && let Some(suspended) = suspended.take()
        {
            // Signal only after registering the child waker. This poll still
            // returns Pending if another worker releases the child immediately.
            suspended.send(()).expect("request waits for every child to suspend");
        }
        result
    })
    .await
    .expect("request releases every child before it finishes");
    mix_request_work(value, SUSPENDED_WORK)
}

async fn resume_children<H>(mut children: [(H, oneshot::Receiver<()>, oneshot::Sender<()>); INNER_OPERATIONS]) -> [H; INNER_OPERATIONS] {
    for (_, suspended, _) in &mut children {
        suspended.await.expect("every child signals after registering its waker");
    }
    children.map(|(handle, _, resume)| {
        resume.send(()).expect("child waits until its request releases it");
        handle
    })
}

async fn arty_suspended_core(cx: Context, cache: Cache) {
    let scheduler = cx.scheduler();
    let handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| {
        scheduler.spawn_anywhere((cx.clone(), cache.clone()), |(cx, cache)| arty_suspended_outer(cx, cache))
    });
    for handle in handles {
        handle.await.expect("benchmark request task finishes before shutdown");
    }
}

async fn arty_suspended_outer(cx: Context, cache: Cache) {
    let scheduler = cx.scheduler();
    let children = std::array::from_fn::<_, INNER_OPERATIONS, _>(|operation| {
        let (suspend, suspended) = oneshot::channel();
        let (resume, resumed) = oneshot::channel();
        let cache = cache.clone();
        let handle = scheduler.spawn(move |_| suspended_request_work(cache, operation, suspend, resumed));
        (handle, suspended, resume)
    });
    for handle in resume_children(children).await {
        black_box(handle.await.expect("benchmark inner task finishes before shutdown"));
    }
}

fn arty_request_suspend(workers: usize) -> ArtyCase {
    let case = ArtyCase::new(workers);
    for _ in 0..workers {
        black_box(case.run_request_suspend());
    }
    case
}

fn arty_request_work(workers: usize, rounds: usize) -> ArtyCase {
    let case = ArtyCase::new(workers);
    for _ in 0..workers {
        black_box(case.run_request_work(rounds));
    }
    case
}

async fn arty_core(cx: Context, cache: Cache) {
    let scheduler = cx.scheduler();
    let handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| {
        scheduler.spawn_anywhere((cx.clone(), cache.clone()), |(cx, cache)| arty_outer(cx, cache))
    });
    for handle in handles {
        handle.await.expect("benchmark request task finishes before shutdown");
    }
}

fn arty_requests(workers: usize) -> ArtyCase {
    let case = ArtyCase::new(workers);
    for _ in 0..workers {
        black_box(case.run_requests());
    }
    case
}

#[derive(Debug)]
struct ArtyNestedRetainedWakerCase {
    runtime: Runtime,
}

impl ArtyNestedRetainedWakerCase {
    fn new(workers: usize) -> Self {
        let case = Self {
            runtime: Runtime::builder()
                .workers(WorkersPolicy::exactly(workers))
                .build()
                .expect("benchmark runtime construction must succeed"),
        };
        for _ in 0..workers {
            black_box(case.run(CONCURRENCY));
        }
        case
    }

    fn run(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        let scheduler = self.runtime.scheduler();
        let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
        let start = Instant::now();
        for handle in &mut handles {
            *handle = Some(scheduler.spawn_anywhere((), arty_retained_waker_outer));
        }
        futures::executor::block_on(async move {
            for handle in handles {
                let Some(handle) = handle else {
                    unreachable!("all benchmark outer task slots are filled");
                };
                handle.await.expect("benchmark outer task finishes before shutdown");
            }
        });
        start.elapsed()
    }
}

async fn arty_outer(cx: Context, cache: Cache) {
    let scheduler = cx.scheduler().clone();
    let mut handles = std::array::from_fn::<_, INNER_OPERATIONS, _>(|_| None);
    for (operation, handle) in handles.iter_mut().enumerate() {
        let cache = cache.clone();
        *handle = Some(scheduler.spawn(move |_| async move { cache.touch(operation) }));
    }

    for handle in handles {
        let Some(handle) = handle else {
            unreachable!("all benchmark inner task slots are filled");
        };
        handle.await.expect("benchmark inner task finishes before shutdown");
    }
}

async fn arty_retained_waker_outer(cx: Context, (): ()) {
    let scheduler = cx.scheduler();
    let mut handles = std::array::from_fn::<_, INNER_OPERATIONS, _>(|_| None);
    for handle in &mut handles {
        let future = RetainedWaker::default();
        *handle = Some(scheduler.spawn(move |_| future));
    }

    for handle in handles {
        let Some(handle) = handle else {
            unreachable!("all benchmark inner task slots are filled");
        };
        handle.await.expect("benchmark inner task finishes before shutdown");
    }
}

#[derive(Debug)]
struct TokioCase {
    runtime: tokio::runtime::Runtime,
    cache: Cache,
}

impl TokioCase {
    fn new(workers: usize) -> Self {
        let case = Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(workers)
                .build()
                .expect("benchmark runtime construction must succeed"),
            cache: Cache::new(),
        };
        for _ in 0..workers {
            black_box(case.run(CONCURRENCY));
        }
        case
    }

    fn run(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        let start = Instant::now();
        let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
        for handle in &mut handles {
            *handle = Some(self.runtime.spawn(tokio_outer(self.cache.clone())));
        }
        futures::executor::block_on(async move {
            for handle in handles {
                let Some(handle) = handle else {
                    unreachable!("all benchmark outer task slots are filled");
                };
                handle.await.expect("benchmark outer task finishes before shutdown");
            }
        });
        start.elapsed()
    }

    fn run_requests(&self) -> Duration {
        let start = Instant::now();
        let core = self.runtime.spawn(tokio_core(self.cache.clone()));
        futures::executor::block_on(core).expect("benchmark core task finishes before shutdown");
        start.elapsed()
    }

    fn run_request_work(&self, rounds: usize) -> Duration {
        let start = Instant::now();
        let core = self.runtime.spawn(tokio_work_core(self.cache.clone(), rounds));
        futures::executor::block_on(core).expect("benchmark core task finishes before shutdown");
        start.elapsed()
    }

    fn run_request_suspend(&self) -> Duration {
        let start = Instant::now();
        let core = self.runtime.spawn(tokio_suspended_core(self.cache.clone()));
        futures::executor::block_on(core).expect("benchmark core task finishes before shutdown");
        start.elapsed()
    }
}

async fn tokio_suspended_core(cache: Cache) {
    let handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| tokio::spawn(tokio_suspended_outer(cache.clone())));
    for handle in handles {
        handle.await.expect("benchmark request task finishes before shutdown");
    }
}

async fn tokio_suspended_outer(cache: Cache) {
    let children = std::array::from_fn::<_, INNER_OPERATIONS, _>(|operation| {
        let (suspend, suspended) = oneshot::channel();
        let (resume, resumed) = oneshot::channel();
        let handle = tokio::spawn(suspended_request_work(cache.clone(), operation, suspend, resumed));
        (handle, suspended, resume)
    });
    for handle in resume_children(children).await {
        black_box(handle.await.expect("benchmark inner task finishes before shutdown"));
    }
}

fn tokio_request_suspend(workers: usize) -> TokioCase {
    let case = TokioCase::new(workers);
    for _ in 0..workers {
        black_box(case.run_request_suspend());
    }
    case
}

async fn tokio_work_core(cache: Cache, rounds: usize) {
    let handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| tokio::spawn(tokio_work_outer(cache.clone(), rounds)));
    for handle in handles {
        handle.await.expect("benchmark request task finishes before shutdown");
    }
}

async fn tokio_work_outer(cache: Cache, rounds: usize) {
    let handles = std::array::from_fn::<_, INNER_OPERATIONS, _>(|operation| {
        let cache = cache.clone();
        tokio::spawn(async move { request_work(&cache, operation, rounds) })
    });
    for handle in handles {
        black_box(handle.await.expect("benchmark inner task finishes before shutdown"));
    }
}

fn tokio_request_work(workers: usize, rounds: usize) -> TokioCase {
    let case = TokioCase::new(workers);
    for _ in 0..workers {
        black_box(case.run_request_work(rounds));
    }
    case
}

async fn tokio_core(cache: Cache) {
    let handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| tokio::spawn(tokio_outer(cache.clone())));
    for handle in handles {
        handle.await.expect("benchmark request task finishes before shutdown");
    }
}

fn tokio_requests(workers: usize) -> TokioCase {
    let case = TokioCase::new(workers);
    for _ in 0..workers {
        black_box(case.run_requests());
    }
    case
}

#[derive(Debug)]
struct TokioNestedRetainedWakerCase {
    runtime: tokio::runtime::Runtime,
}

impl TokioNestedRetainedWakerCase {
    fn new(workers: usize) -> Self {
        let case = Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(workers)
                .build()
                .expect("benchmark runtime construction must succeed"),
        };
        for _ in 0..workers {
            black_box(case.run(CONCURRENCY));
        }
        case
    }

    fn run(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
        let start = Instant::now();
        for handle in &mut handles {
            *handle = Some(self.runtime.spawn(tokio_retained_waker_outer()));
        }
        futures::executor::block_on(async move {
            for handle in handles {
                let Some(handle) = handle else {
                    unreachable!("all benchmark outer task slots are filled");
                };
                handle.await.expect("benchmark outer task finishes before shutdown");
            }
        });
        start.elapsed()
    }
}

async fn tokio_outer(cache: Cache) {
    let mut handles = std::array::from_fn::<_, INNER_OPERATIONS, _>(|_| None);
    for (operation, handle) in handles.iter_mut().enumerate() {
        let cache = cache.clone();
        *handle = Some(tokio::spawn(async move { cache.touch(operation) }));
    }

    for handle in handles {
        let Some(handle) = handle else {
            unreachable!("all benchmark inner task slots are filled");
        };
        handle.await.expect("benchmark inner task finishes before shutdown");
    }
}

async fn tokio_retained_waker_outer() {
    let mut handles = std::array::from_fn::<_, INNER_OPERATIONS, _>(|_| None);
    for handle in &mut handles {
        let future = RetainedWaker::default();
        *handle = Some(tokio::spawn(future));
    }

    for handle in handles {
        let Some(handle) = handle else {
            unreachable!("all benchmark inner task slots are filled");
        };
        handle.await.expect("benchmark inner task finishes before shutdown");
    }
}

#[derive(Debug, Default)]
struct RetainedWaker {
    retained: Option<Waker>,
}

impl Future for RetainedWaker {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<()> {
        if self.retained.take().is_some() {
            Poll::Ready(())
        } else {
            self.retained = Some(cx.waker().clone());
            self.retained.as_ref().expect("waker was retained immediately above").wake_by_ref();
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

fn measure_native<T>(workload: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "linux")]
    gungraun::client_requests::callgrind::start_instrumentation();
    let result = workload();
    #[cfg(target_os = "linux")]
    gungraun::client_requests::callgrind::stop_instrumentation();
    result
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ARTY.group_name());
    group.throughput(Throughput::Elements(
        u64::try_from(INNER_OPERATIONS * CONCURRENCY).expect("benchmark operation count fits in u64"),
    ));
    for workers in WORKERS {
        let name = format!("concurrency_w{workers}");
        group.bench_function(BenchmarkId::new(ARTY.benchmark_name(), &name), |bencher| {
            let case = ArtyCase::new(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run(CONCURRENCY);
                }
                elapsed
            });
        });
        group.bench_function(BenchmarkId::new(TOKIO.benchmark_name(), &name), |bencher| {
            let case = TokioCase::new(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run(CONCURRENCY);
                }
                elapsed
            });
        });
    }
    group.finish();

    let mut group = criterion.benchmark_group(ARTY_REQUEST_FANOUT.group_name());
    group.throughput(Throughput::Elements(
        u64::try_from(INNER_OPERATIONS * CONCURRENCY).expect("benchmark operation count fits in u64"),
    ));
    for workers in WORKERS {
        let name = format!("concurrency_w{workers}");
        group.bench_function(BenchmarkId::new(ARTY_REQUEST_FANOUT.benchmark_name(), &name), |bencher| {
            let case = arty_requests(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run_requests();
                }
                elapsed
            });
        });
        group.bench_function(BenchmarkId::new(TOKIO_REQUEST_FANOUT.benchmark_name(), &name), |bencher| {
            let case = tokio_requests(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run_requests();
                }
                elapsed
            });
        });
    }
    group.finish();

    let mut group = criterion.benchmark_group(ARTY_REQUEST_WORK.group_name());
    group.throughput(Throughput::Elements(
        u64::try_from(INNER_OPERATIONS * CONCURRENCY).expect("benchmark operation count fits in u64"),
    ));
    for rounds in REQUEST_WORK {
        for workers in WORKERS {
            let name = format!("work{rounds}_w{workers}");
            group.bench_function(BenchmarkId::new(ARTY_REQUEST_WORK.benchmark_name(), &name), |bencher| {
                let case = arty_request_work(workers, rounds);
                bencher.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        elapsed += case.run_request_work(rounds);
                    }
                    elapsed
                });
            });
            group.bench_function(BenchmarkId::new(TOKIO_REQUEST_WORK.benchmark_name(), &name), |bencher| {
                let case = tokio_request_work(workers, rounds);
                bencher.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        elapsed += case.run_request_work(rounds);
                    }
                    elapsed
                });
            });
        }
    }
    group.finish();

    let mut group = criterion.benchmark_group(ARTY_REQUEST_SUSPEND.group_name());
    group.throughput(Throughput::Elements(
        u64::try_from(INNER_OPERATIONS * CONCURRENCY).expect("benchmark operation count fits in u64"),
    ));
    for workers in WORKERS {
        let name = format!("concurrency_w{workers}");
        group.bench_function(BenchmarkId::new(ARTY_REQUEST_SUSPEND.benchmark_name(), &name), |bencher| {
            let case = arty_request_suspend(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run_request_suspend();
                }
                elapsed
            });
        });
        group.bench_function(BenchmarkId::new(TOKIO_REQUEST_SUSPEND.benchmark_name(), &name), |bencher| {
            let case = tokio_request_suspend(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run_request_suspend();
                }
                elapsed
            });
        });
    }
    group.finish();

    let mut group = criterion.benchmark_group(ARTY_NESTED_RETAINED_WAKER.group_name());
    group.throughput(Throughput::Elements(
        u64::try_from(INNER_OPERATIONS * CONCURRENCY).expect("benchmark operation count fits in u64"),
    ));
    for workers in WORKERS {
        let name = format!("concurrency_w{workers}");
        group.bench_function(BenchmarkId::new(ARTY_NESTED_RETAINED_WAKER.benchmark_name(), &name), |bencher| {
            let case = ArtyNestedRetainedWakerCase::new(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run(CONCURRENCY);
                }
                elapsed
            });
        });
        group.bench_function(BenchmarkId::new(TOKIO_NESTED_RETAINED_WAKER.benchmark_name(), &name), |bencher| {
            let case = TokioNestedRetainedWakerCase::new(workers);
            bencher.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    elapsed += case.run(CONCURRENCY);
                }
                elapsed
            });
        });
    }
    group.finish();
}

#[benchmark(ARTY, "arty_contention/relocated_cache", "Arty")]
#[bench::concurrency_w1(Box::new(ArtyCase::new(1)), 10)]
#[bench::concurrency_w8(Box::new(ArtyCase::new(8)), 10)]
fn arty_workload<S: std::borrow::Borrow<ArtyCase>>(owned_input: S, concurrency: u64) -> (Duration, S) {
    let result = measure_native(|| {
        let case = owned_input.borrow();
        case.run(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"))
    });
    (result, owned_input)
}

#[benchmark(TOKIO, "arty_contention/relocated_cache", "Tokio")]
#[bench::concurrency_w1(Box::new(TokioCase::new(1)), 10)]
#[bench::concurrency_w8(Box::new(TokioCase::new(8)), 10)]
fn tokio_workload<S: std::borrow::Borrow<TokioCase>>(owned_input: S, concurrency: u64) -> (Duration, S) {
    let result = measure_native(|| {
        let case = owned_input.borrow();
        case.run(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"))
    });
    (result, owned_input)
}

#[benchmark(ARTY_NESTED_RETAINED_WAKER, "arty_contention/nested_retained_waker", "Arty")]
#[bench::concurrency_w1(Box::new(ArtyNestedRetainedWakerCase::new(1)), 10)]
#[bench::concurrency_w8(Box::new(ArtyNestedRetainedWakerCase::new(8)), 10)]
fn arty_nested_retained_waker_workload<S: std::borrow::Borrow<ArtyNestedRetainedWakerCase>>(
    owned_input: S,
    concurrency: u64,
) -> (Duration, S) {
    let result = measure_native(|| {
        let case = owned_input.borrow();
        case.run(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"))
    });
    (result, owned_input)
}

#[benchmark(TOKIO_NESTED_RETAINED_WAKER, "arty_contention/nested_retained_waker", "Tokio")]
#[bench::concurrency_w1(Box::new(TokioNestedRetainedWakerCase::new(1)), 10)]
#[bench::concurrency_w8(Box::new(TokioNestedRetainedWakerCase::new(8)), 10)]
fn tokio_nested_retained_waker_workload<S: std::borrow::Borrow<TokioNestedRetainedWakerCase>>(
    owned_input: S,
    concurrency: u64,
) -> (Duration, S) {
    let result = measure_native(|| {
        let case = owned_input.borrow();
        case.run(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"))
    });
    (result, owned_input)
}

#[benchmark(ARTY_REQUEST_FANOUT, "arty_contention/request_fanout", "Arty")]
#[bench::concurrency_w1(Box::new(arty_requests(1)))]
#[bench::concurrency_w8(Box::new(arty_requests(8)))]
fn arty_request_fanout_workload<S: std::borrow::Borrow<ArtyCase>>(owned_input: S) -> (Duration, S) {
    let result = measure_native(|| owned_input.borrow().run_requests());
    (result, owned_input)
}

#[benchmark(TOKIO_REQUEST_FANOUT, "arty_contention/request_fanout", "Tokio")]
#[bench::concurrency_w1(Box::new(tokio_requests(1)))]
#[bench::concurrency_w8(Box::new(tokio_requests(8)))]
fn tokio_request_fanout_workload<S: std::borrow::Borrow<TokioCase>>(owned_input: S) -> (Duration, S) {
    let result = measure_native(|| owned_input.borrow().run_requests());
    (result, owned_input)
}

#[benchmark(ARTY_REQUEST_WORK, "arty_contention/request_work", "Arty")]
#[bench::work16_w1(Box::new(arty_request_work(1, 16)), 16)]
#[bench::work16_w8(Box::new(arty_request_work(8, 16)), 16)]
#[bench::work256_w1(Box::new(arty_request_work(1, 256)), 256)]
#[bench::work256_w8(Box::new(arty_request_work(8, 256)), 256)]
fn arty_request_work_workload<S: std::borrow::Borrow<ArtyCase>>(owned_input: S, rounds: usize) -> (Duration, S) {
    let result = measure_native(|| owned_input.borrow().run_request_work(rounds));
    (result, owned_input)
}

#[benchmark(TOKIO_REQUEST_WORK, "arty_contention/request_work", "Tokio")]
#[bench::work16_w1(Box::new(tokio_request_work(1, 16)), 16)]
#[bench::work16_w8(Box::new(tokio_request_work(8, 16)), 16)]
#[bench::work256_w1(Box::new(tokio_request_work(1, 256)), 256)]
#[bench::work256_w8(Box::new(tokio_request_work(8, 256)), 256)]
fn tokio_request_work_workload<S: std::borrow::Borrow<TokioCase>>(owned_input: S, rounds: usize) -> (Duration, S) {
    let result = measure_native(|| owned_input.borrow().run_request_work(rounds));
    (result, owned_input)
}

#[benchmark(ARTY_REQUEST_SUSPEND, "arty_contention/request_suspend", "Arty")]
#[bench::concurrency_w1(Box::new(arty_request_suspend(1)))]
#[bench::concurrency_w8(Box::new(arty_request_suspend(8)))]
fn arty_request_suspend_workload<S: std::borrow::Borrow<ArtyCase>>(owned_input: S) -> (Duration, S) {
    let result = measure_native(|| owned_input.borrow().run_request_suspend());
    (result, owned_input)
}

#[benchmark(TOKIO_REQUEST_SUSPEND, "arty_contention/request_suspend", "Tokio")]
#[bench::concurrency_w1(Box::new(tokio_request_suspend(1)))]
#[bench::concurrency_w8(Box::new(tokio_request_suspend(8)))]
fn tokio_request_suspend_workload<S: std::borrow::Borrow<TokioCase>>(owned_input: S) -> (Duration, S) {
    let result = measure_native(|| owned_input.borrow().run_request_suspend());
    (result, owned_input)
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
                .entry_point(EntryPoint::None)
                .args(["--branch-sim=yes", "--instr-atstart=no"])
                .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
        );
    },
    benchmarks = [
        ARTY,
        TOKIO,
        ARTY_NESTED_RETAINED_WAKER,
        TOKIO_NESTED_RETAINED_WAKER,
        ARTY_REQUEST_FANOUT,
        TOKIO_REQUEST_FANOUT,
        ARTY_REQUEST_WORK,
        TOKIO_REQUEST_WORK,
        ARTY_REQUEST_SUSPEND,
        TOKIO_REQUEST_SUSPEND,
    ],
);

#[cfg(not(target_os = "linux"))]
metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [
        ARTY,
        TOKIO,
        ARTY_NESTED_RETAINED_WAKER,
        TOKIO_NESTED_RETAINED_WAKER,
        ARTY_REQUEST_FANOUT,
        TOKIO_REQUEST_FANOUT,
        ARTY_REQUEST_WORK,
        TOKIO_REQUEST_WORK,
        ARTY_REQUEST_SUSPEND,
        TOKIO_REQUEST_SUSPEND,
    ],
);
