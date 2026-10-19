// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-thread watch contention with recording disabled.
//!
//! Churn subscribes, polls a false predicate once, then cancels the wait and
//! drops the receiver while each worker retains a configurable pending backlog.
//! Read/clone measures the shutdown-check path; broadcast measures publication
//! and completion of already-registered waits. The std control is a minimal
//! mutex-backed watch reference, not a primitive provided by the standard library.
//!
//! A persistent local `bench_on_threadpool` harness is used because no shared
//! threadpool helper exists in this workspace. Thread creation, backlog setup
//! and teardown are outside timing. Each timed iteration performs one operation
//! per worker; Criterion throughput counts all workers. CPU placement is left
//! to the OS. These contention scenarios intentionally have no Callgrind pair.
//! Worker counts above the available CPU budget are skipped, including the
//! 32-, 172- and 176-worker configurations intended for larger hosts.

#![allow(missing_docs, reason = "benchmark code")]

use std::collections::VecDeque;
use std::hint::black_box;
use std::pin::{Pin, pin};
use std::sync::{Arc, Barrier, Mutex, PoisonError, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use performables::sync::channel;

const WORKERS: [usize; 7] = [1, 2, 4, 8, 32, 172, 176];
const BACKLOG: [usize; 3] = [0, 64, 256];
const FANOUT: [usize; 3] = [8, 512, 2_048];
type Pending = Vec<Pin<Box<dyn Future<Output = ()>>>>;

fn worker_counts() -> impl Iterator<Item = usize> {
    let available = thread::available_parallelism()
        .expect("watch contention benchmarks require an available CPU budget")
        .get();
    WORKERS.into_iter().filter(move |workers| *workers <= available)
}

struct Threadpool {
    commands: Vec<mpsc::Sender<Option<u64>>>,
    workers: Vec<thread::JoinHandle<()>>,
    boundary: Arc<Barrier>,
}

impl Threadpool {
    fn new<S, L>(
        workers: usize,
        shared: S,
        prepare: impl Fn(&S) -> L + Send + Sync + 'static,
        operation: impl Fn(&S, &mut L) + Send + Sync + 'static,
    ) -> Self
    where
        S: Send + Sync + 'static,
    {
        let boundary = Arc::new(Barrier::new(workers + 1));
        let shared = Arc::new(shared);
        let prepare = Arc::new(prepare);
        let operation = Arc::new(operation);
        let mut commands = Vec::with_capacity(workers);
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let (command, received) = mpsc::channel();
            commands.push(command);
            let boundary = Arc::clone(&boundary);
            let shared = Arc::clone(&shared);
            let prepare = Arc::clone(&prepare);
            let operation = Arc::clone(&operation);
            handles.push(thread::spawn(move || {
                let mut local = prepare(&shared);
                boundary.wait();
                while let Some(iterations) = received.recv().expect("the benchmark owns every command sender") {
                    boundary.wait();
                    for _ in 0..iterations {
                        operation(&shared, &mut local);
                    }
                    boundary.wait();
                }
            }));
        }
        boundary.wait();
        Self {
            commands,
            workers: handles,
            boundary,
        }
    }

    fn sample(&self, iterations: u64) -> Duration {
        for command in &self.commands {
            command
                .send(Some(iterations))
                .expect("benchmark workers remain alive until teardown");
        }
        // Include dispatch barriers equally for every control, amortized over the sample.
        let started = Instant::now();
        self.boundary.wait();
        self.boundary.wait();
        started.elapsed()
    }
}

impl Drop for Threadpool {
    fn drop(&mut self) {
        for command in &self.commands {
            command.send(None).expect("benchmark workers remain alive until teardown");
        }
        for worker in self.workers.drain(..) {
            worker.join().expect("a benchmark worker must not panic");
        }
    }
}

fn bench_on_threadpool<S, L>(
    bencher: &mut criterion::Bencher<'_>,
    workers: usize,
    shared: S,
    prepare: impl Fn(&S) -> L + Send + Sync + 'static,
    operation: impl Fn(&S, &mut L) + Send + Sync + 'static,
) where
    S: Send + Sync + 'static,
{
    let pool = Threadpool::new(workers, shared, prepare, operation);
    bencher.iter_custom(|iterations| pool.sample(iterations));
}

#[derive(Default)]
struct StandardState {
    value: bool,
    senders: usize,
    receivers: usize,
}

struct StandardShared {
    state: Mutex<StandardState>,
    waiters: Mutex<VecDeque<Arc<Waker>>>,
}

struct StandardSender(Arc<StandardShared>);
struct StandardReceiver(Arc<StandardShared>);

impl StandardSender {
    fn new() -> Self {
        Self(Arc::new(StandardShared {
            state: Mutex::new(StandardState {
                senders: 1,
                ..StandardState::default()
            }),
            waiters: Mutex::new(VecDeque::new()),
        }))
    }

    fn subscribe(&self) -> StandardReceiver {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).receivers += 1;
        StandardReceiver(Arc::clone(&self.0))
    }

    fn value(&self) -> bool {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).value
    }

    fn send(&self) {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).value = true;
        let waiters = std::mem::take(&mut *self.0.waiters.lock().unwrap_or_else(PoisonError::into_inner));
        for waiter in waiters {
            waiter.wake_by_ref();
        }
    }
}

impl Clone for StandardSender {
    fn clone(&self) -> Self {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).senders += 1;
        Self(Arc::clone(&self.0))
    }
}

impl Drop for StandardSender {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).senders -= 1;
    }
}

impl Drop for StandardReceiver {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).receivers -= 1;
    }
}

struct StandardWait<'a> {
    receiver: &'a StandardReceiver,
    queued: Option<Arc<Waker>>,
}

impl StandardReceiver {
    fn wait(&self) -> StandardWait<'_> {
        StandardWait {
            receiver: self,
            queued: None,
        }
    }
}

impl Future for StandardWait<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let shared = &self.receiver.0;
        if shared.state.lock().unwrap_or_else(PoisonError::into_inner).value {
            return Poll::Ready(());
        }
        if self.queued.is_none() {
            let waiter = Arc::new(cx.waker().clone());
            let mut waiters = shared.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            if shared.state.lock().unwrap_or_else(PoisonError::into_inner).value {
                return Poll::Ready(());
            }
            waiters.push_back(Arc::clone(&waiter));
            drop(waiters);
            self.queued = Some(waiter);
        }
        Poll::Pending
    }
}

impl Drop for StandardWait<'_> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.queued {
            let mut waiters = self.receiver.0.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(index) = waiters.iter().position(|queued| Arc::ptr_eq(queued, waiter)) {
                waiters.remove(index);
            }
        }
    }
}

fn pending_performables(sender: &channel::WatchSender<bool>, count: usize) -> Pending {
    let mut context = Context::from_waker(Waker::noop());
    (0..count)
        .map(|_| {
            let receiver = sender.subscribe();
            let mut wait: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async move {
                let _value = receiver
                    .wait_for_async(|value| *value)
                    .await
                    .expect("the benchmark retains its sender");
            });
            assert!(wait.as_mut().poll(&mut context).is_pending());
            wait
        })
        .collect()
}

fn pending_tokio(sender: &tokio::sync::watch::Sender<bool>, count: usize) -> Pending {
    let mut context = Context::from_waker(Waker::noop());
    (0..count)
        .map(|_| {
            let mut receiver = sender.subscribe();
            let mut wait: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async move {
                let _value = receiver.wait_for(|value| *value).await.expect("the benchmark retains its sender");
            });
            assert!(wait.as_mut().poll(&mut context).is_pending());
            wait
        })
        .collect()
}

fn pending_standard(sender: &StandardSender, count: usize) -> Pending {
    let mut context = Context::from_waker(Waker::noop());
    (0..count)
        .map(|_| {
            let receiver = sender.subscribe();
            let mut wait: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async move {
                receiver.wait().await;
            });
            assert!(wait.as_mut().poll(&mut context).is_pending());
            wait
        })
        .collect()
}

fn churn_performables(sender: &channel::WatchSender<bool>, _: &mut Pending) {
    let receiver = black_box(sender).subscribe();
    // Stack-pin measured waits so allocator noise does not hide registration cost.
    let mut wait = pin!(receiver.wait_for_async(|value| *value));
    assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
}

fn churn_tokio(sender: &tokio::sync::watch::Sender<bool>, _: &mut Pending) {
    let mut receiver = black_box(sender).subscribe();
    let mut wait = pin!(receiver.wait_for(|value| *value));
    assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
}

fn churn_standard(sender: &StandardSender, _: &mut Pending) {
    let receiver = black_box(sender).subscribe();
    let mut wait = pin!(receiver.wait());
    assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
}

fn churn(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_watch/churn");
    for workers in worker_counts() {
        group.throughput(Throughput::Elements(u64::try_from(workers).expect("the worker count fits in u64")));
        for backlog in BACKLOG {
            let configuration = format!("t{workers}_pending{}", workers * backlog);
            group.bench_function(BenchmarkId::new("performables", &configuration), |bencher| {
                let (sender, _receiver) = channel::watch(false);
                bench_on_threadpool(
                    bencher,
                    workers,
                    sender,
                    move |sender| pending_performables(sender, backlog),
                    churn_performables,
                );
            });
            group.bench_function(BenchmarkId::new("tokio", &configuration), |bencher| {
                let (sender, _receiver) = tokio::sync::watch::channel(false);
                bench_on_threadpool(bencher, workers, sender, move |sender| pending_tokio(sender, backlog), churn_tokio);
            });
            group.bench_function(BenchmarkId::new("std_mutex_reference", &configuration), |bencher| {
                bench_on_threadpool(
                    bencher,
                    workers,
                    StandardSender::new(),
                    move |sender| pending_standard(sender, backlog),
                    churn_standard,
                );
            });
        }
    }
    group.finish();
}

fn read_clone(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_watch/read_clone");
    for workers in worker_counts() {
        group.throughput(Throughput::Elements(u64::try_from(workers).expect("the worker count fits in u64")));
        group.bench_function(BenchmarkId::new("performables", workers), |bencher| {
            let (sender, _receiver) = channel::watch(false);
            bench_on_threadpool(
                bencher,
                workers,
                sender,
                |_| (),
                |sender, ()| {
                    let sender = sender.clone();
                    black_box(*sender.borrow());
                },
            );
        });
        group.bench_function(BenchmarkId::new("tokio", workers), |bencher| {
            let (sender, _receiver) = tokio::sync::watch::channel(false);
            bench_on_threadpool(
                bencher,
                workers,
                sender,
                |_| (),
                |sender, ()| {
                    let sender = sender.clone();
                    black_box(*sender.borrow());
                },
            );
        });
        group.bench_function(BenchmarkId::new("std_mutex_reference", workers), |bencher| {
            bench_on_threadpool(
                bencher,
                workers,
                StandardSender::new(),
                |_| (),
                |sender, ()| {
                    let sender = sender.clone();
                    black_box(sender.value());
                },
            );
        });
    }
    group.finish();
}

fn complete(waits: &mut Pending) {
    let mut context = Context::from_waker(Waker::noop());
    for wait in waits {
        assert!(wait.as_mut().poll(&mut context).is_ready());
    }
}

fn broadcast(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_watch/broadcast");
    for count in FANOUT {
        group.throughput(Throughput::Elements(u64::try_from(count).expect("the waiter count fits in u64")));
        group.bench_function(BenchmarkId::new("performables", count), |bencher| {
            bencher.iter_batched_ref(
                || {
                    let (sender, _receiver) = channel::watch(false);
                    let waits = pending_performables(&sender, count);
                    (sender, waits)
                },
                |(sender, waits)| {
                    sender.send_replace(true);
                    complete(waits);
                },
                BatchSize::PerIteration,
            );
        });
        group.bench_function(BenchmarkId::new("tokio", count), |bencher| {
            bencher.iter_batched_ref(
                || {
                    let (sender, _receiver) = tokio::sync::watch::channel(false);
                    let waits = pending_tokio(&sender, count);
                    (sender, waits)
                },
                |(sender, waits)| {
                    sender.send_replace(true);
                    complete(waits);
                },
                BatchSize::PerIteration,
            );
        });
        group.bench_function(BenchmarkId::new("std_mutex_reference", count), |bencher| {
            bencher.iter_batched_ref(
                || {
                    let sender = StandardSender::new();
                    let waits = pending_standard(&sender, count);
                    (sender, waits)
                },
                |(sender, waits)| {
                    sender.send();
                    complete(waits);
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

fn benchmarks(criterion: &mut Criterion) {
    #[cfg(feature = "seismograph")]
    seismograph::recorder(seismograph::recorder::Configuration::default());
    read_clone(criterion);
    churn(criterion);
    broadcast(criterion);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
