// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arty and Tokio contention benchmarks for common nested-task patterns.
//!
//! Each case submits one outer task that starts ten inner operations.
//! The relocated-cache case uses `black_box` on each cache access.
//! This prevents the compiler from removing the operation that the benchmark measures.
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
//! Gungraun results give a rough view of which instructions run. Its cases use
//! one prewarmed worker and collect inside that worker. Callgrind cannot model
//! how tasks share workers or compete across workers. Use Criterion for
//! multi-worker runtime comparisons.

#![expect(
    unreachable_pub,
    reason = "Triggered by metabench expansion for benchmark-local Gungraun configuration."
)]

use std::hint::black_box;
use std::pin::Pin;
use std::task::{Context as PollContext, Poll, Waker};
use std::time::{Duration, Instant};

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Context;
use criterion::{BenchmarkId, Criterion, Throughput};
use gungraun::LibraryBenchmarkConfig;
#[cfg(target_os = "linux")]
use gungraun::client_requests::callgrind;
#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, EntryPoint};
use metabench::benchmark;
use performables::arc::{Arc, PerThread};
use performables::sync::mutex::Mutex;
use thread_aware::{Thread, ThreadAware, Unaware};

const INNER_OPERATIONS: usize = 10;
const CONCURRENCY: usize = 10;
const CACHE_ENTRIES: usize = 8;
const WORKERS: [usize; 2] = [1, 8];

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

    fn run_instrumented(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        let caches = std::array::from_fn::<_, CONCURRENCY, _>(|_| self.cache.clone());
        futures::executor::block_on(self.runtime.scheduler().spawn_anywhere(
            Unaware((caches, concurrency)),
            |cx, Unaware((mut caches, concurrency))| async move {
                toggle_callgrind_collection();
                let start = Instant::now();
                let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
                for cache in caches.iter_mut().take(concurrency) {
                    cache.relocate(None, cx.thread());
                }
                for (cache, handle) in caches.into_iter().zip(handles.iter_mut()).take(concurrency) {
                    *handle = Some(cx.scheduler().spawn(move |child| arty_outer(child, cache)));
                }
                for handle in handles.into_iter().take(concurrency) {
                    handle
                        .expect("all benchmark outer task slots are filled")
                        .await
                        .expect("benchmark outer task finishes before shutdown");
                }
                let elapsed = start.elapsed();
                toggle_callgrind_collection();
                elapsed
            },
        ))
        .expect("benchmark controller finishes before shutdown")
    }
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

    fn run_instrumented(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        futures::executor::block_on(self.runtime.scheduler().spawn_anywhere(concurrency, |cx, concurrency| async move {
            toggle_callgrind_collection();
            let start = Instant::now();
            let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
            for handle in handles.iter_mut().take(concurrency) {
                *handle = Some(cx.scheduler().spawn(|child| arty_retained_waker_outer(child, ())));
            }
            for handle in handles.into_iter().take(concurrency) {
                handle
                    .expect("all benchmark outer task slots are filled")
                    .await
                    .expect("benchmark outer task finishes before shutdown");
            }
            let elapsed = start.elapsed();
            toggle_callgrind_collection();
            elapsed
        }))
        .expect("benchmark controller finishes before shutdown")
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

    fn run_instrumented(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        let cache = self.cache.clone();
        futures::executor::block_on(self.runtime.spawn(async move {
            toggle_callgrind_collection();
            let start = Instant::now();
            let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
            for handle in handles.iter_mut().take(concurrency) {
                *handle = Some(tokio::spawn(tokio_outer(cache.clone())));
            }
            for handle in handles.into_iter().take(concurrency) {
                handle
                    .expect("all benchmark outer task slots are filled")
                    .await
                    .expect("benchmark outer task finishes before shutdown");
            }
            let elapsed = start.elapsed();
            toggle_callgrind_collection();
            elapsed
        }))
        .expect("benchmark controller task does not panic")
    }
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

    fn run_instrumented(&self, concurrency: usize) -> Duration {
        assert_eq!(concurrency, CONCURRENCY, "benchmark uses its fixed concurrency");
        futures::executor::block_on(self.runtime.spawn(async move {
            toggle_callgrind_collection();
            let start = Instant::now();
            let mut handles = std::array::from_fn::<_, CONCURRENCY, _>(|_| None);
            for handle in handles.iter_mut().take(concurrency) {
                *handle = Some(tokio::spawn(tokio_retained_waker_outer()));
            }
            for handle in handles.into_iter().take(concurrency) {
                handle
                    .expect("all benchmark outer task slots are filled")
                    .await
                    .expect("benchmark outer task finishes before shutdown");
            }
            let elapsed = start.elapsed();
            toggle_callgrind_collection();
            elapsed
        }))
        .expect("benchmark controller task does not panic")
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

fn toggle_callgrind_collection() {
    #[cfg(target_os = "linux")]
    callgrind::toggle_collect();
}

#[cfg(target_os = "linux")]
fn worker_contention_config() -> LibraryBenchmarkConfig {
    let mut callgrind = Callgrind::default();
    callgrind
        .args(["--branch-sim=yes", "--instr-atstart=no", "--collect-atstart=no"])
        .entry_point(EntryPoint::None);
    let mut config = LibraryBenchmarkConfig::default();
    config.tool(callgrind.format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]));
    config
}

#[cfg(not(target_os = "linux"))]
fn worker_contention_config() -> LibraryBenchmarkConfig {
    LibraryBenchmarkConfig::default()
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

#[benchmark(
    ARTY,
    "arty_contention/relocated_cache",
    "Arty",
    gungraun_config = worker_contention_config()
)]
#[bench::concurrency_w1(Box::new(ArtyCase::new(1)), 10)]
fn arty_workload<S: std::borrow::Borrow<ArtyCase>>(owned_input: S, concurrency: u64) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input
        .borrow()
        .run_instrumented(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"));
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[benchmark(
    TOKIO,
    "arty_contention/relocated_cache",
    "Tokio",
    gungraun_config = worker_contention_config()
)]
#[bench::concurrency_w1(Box::new(TokioCase::new(1)), 10)]
fn tokio_workload<S: std::borrow::Borrow<TokioCase>>(owned_input: S, concurrency: u64) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input
        .borrow()
        .run_instrumented(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"));
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[benchmark(
    ARTY_NESTED_RETAINED_WAKER,
    "arty_contention/nested_retained_waker",
    "Arty",
    gungraun_config = worker_contention_config()
)]
#[bench::concurrency_w1(Box::new(ArtyNestedRetainedWakerCase::new(1)), 10)]
fn arty_nested_retained_waker_workload<S: std::borrow::Borrow<ArtyNestedRetainedWakerCase>>(
    owned_input: S,
    concurrency: u64,
) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input
        .borrow()
        .run_instrumented(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"));
    #[cfg(target_os = "linux")]
    callgrind::stop_instrumentation();
    (elapsed, owned_input)
}

#[benchmark(
    TOKIO_NESTED_RETAINED_WAKER,
    "arty_contention/nested_retained_waker",
    "Tokio",
    gungraun_config = worker_contention_config()
)]
#[bench::concurrency_w1(Box::new(TokioNestedRetainedWakerCase::new(1)), 10)]
fn tokio_nested_retained_waker_workload<S: std::borrow::Borrow<TokioNestedRetainedWakerCase>>(
    owned_input: S,
    concurrency: u64,
) -> (Duration, S) {
    #[cfg(target_os = "linux")]
    callgrind::start_instrumentation();
    let elapsed = owned_input
        .borrow()
        .run_instrumented(usize::try_from(concurrency).expect("benchmark concurrency fits in usize"));
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
    benchmarks = [ARTY, TOKIO, ARTY_NESTED_RETAINED_WAKER, TOKIO_NESTED_RETAINED_WAKER],
);

#[cfg(not(target_os = "linux"))]
metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [ARTY, TOKIO, ARTY_NESTED_RETAINED_WAKER, TOKIO_NESTED_RETAINED_WAKER],
);
