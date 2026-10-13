// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uncontended ownership, lock, and ready blocking benchmarks.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::hint::black_box;
use std::pin::pin;
use std::sync::PoisonError;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use criterion::{BatchSize, Criterion};
use performables::arc::Arc;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{self, OneshotReceiver, Receiver, Sender, WatchReceiver, WatchSender};
use performables::sync::lock::RwLock;
use performables::sync::mode;
use performables::sync::mutex::Mutex;

const ARC_NEW_DROP: &str = "performables_uncontended/arc_new_drop";
const ARC_DEREF: &str = "performables_uncontended/arc_deref";
const ARC_CLONE_DROP: &str = "performables_uncontended/arc_clone_drop";
const MUTEX_LOCK: &str = "performables_uncontended/mutex_lock";
const RW_LOCK_READ: &str = "performables_uncontended/rw_lock_read";
const RW_LOCK_WRITE: &str = "performables_uncontended/rw_lock_write";
const READY_BLOCKING: &str = "performables_uncontended/ready_blocking";
// Fixed batches keep fixture setup outside measurements without scaling the
// working set with the speed of the operation.
const READY_BATCH: BatchSize = BatchSize::NumIterations(64);

type Queue = (Sender<u64>, Receiver<u64>);

fn empty_queue(bounded: bool) -> Queue {
    let queue = if bounded { channel::bounded(1) } else { channel::unbounded() };
    queue.0.try_send(7).expect("an empty queue with a live receiver accepts a value");
    queue.1.try_recv().expect("the fixture just buffered a value");
    queue
}

fn ready_queue(bounded: bool) -> Queue {
    let queue = empty_queue(bounded);
    queue.0.try_send(7).expect("an empty queue with a live receiver accepts a value");
    queue
}

#[metabench::benchmark(QUEUE_SEND, READY_BLOCKING, "queue_send")]
#[bench::bounded(&mut empty_queue(true))]
#[bench::unbounded(&mut empty_queue(false))]
fn queue_send(queue: &mut Queue) {
    black_box(&queue.0)
        .send(black_box(7))
        .expect("the fixture provides an empty queue with a live receiver");
}

#[metabench::benchmark(QUEUE_RECV, READY_BLOCKING, "queue_recv")]
#[bench::bounded(&mut ready_queue(true))]
#[bench::unbounded(&mut ready_queue(false))]
fn queue_recv(queue: &mut Queue) -> u64 {
    black_box(&queue.1).recv().expect("the fixture provides a buffered value")
}

#[metabench::benchmark(QUEUE_RECV_TIMEOUT, READY_BLOCKING, "queue_recv_timeout")]
#[bench::bounded(&mut ready_queue(true))]
#[bench::unbounded(&mut ready_queue(false))]
fn queue_recv_timeout(queue: &mut Queue) -> u64 {
    black_box(&queue.1)
        .recv_timeout(Duration::ZERO)
        .expect("an already buffered value completes even at a zero deadline")
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "batched fixtures consume the receiver through a mutable Option reference"
)]
fn ready_oneshot() -> Option<OneshotReceiver<u64>> {
    let (sender, receiver) = channel::oneshot();
    sender.send(7).expect("the fixture retains a live receiver");
    Some(receiver)
}

#[metabench::benchmark(ONESHOT_RECV, READY_BLOCKING, "oneshot_recv")]
#[bench::value(&mut ready_oneshot())]
fn oneshot_recv(receiver: &mut Option<OneshotReceiver<u64>>) -> u64 {
    black_box(receiver.take().expect("each batch supplies a fresh receiver"))
        .recv()
        .expect("the fixture provides a completed oneshot")
}

#[metabench::benchmark(ONESHOT_RECV_TIMEOUT, READY_BLOCKING, "oneshot_recv_timeout")]
#[bench::value(&mut ready_oneshot())]
fn oneshot_recv_timeout(receiver: &mut Option<OneshotReceiver<u64>>) -> u64 {
    black_box(receiver.take().expect("each batch supplies a fresh receiver"))
        .recv_timeout(Duration::ZERO)
        .expect("an already completed oneshot succeeds even at a zero deadline")
}

fn ready_watch() -> (WatchSender<u64>, WatchReceiver<u64>) {
    let pair = channel::watch(0);
    pair.0.send(7).expect("the fixture retains a live receiver");
    pair
}

#[metabench::benchmark(WATCH_CHANGED, READY_BLOCKING, "watch_changed")]
#[bench::value(&mut ready_watch())]
fn watch_changed(pair: &mut (WatchSender<u64>, WatchReceiver<u64>)) {
    black_box(&pair.1).changed().expect("the fixture provides an unobserved version");
}

#[metabench::benchmark(BARRIER_SYNC, READY_BLOCKING, "barrier_sync")]
#[bench::value(&Barrier::<mode::Sync>::new(1))]
fn barrier_sync(barrier: &Barrier) -> bool {
    black_box(barrier).wait().is_leader()
}

#[metabench::benchmark(BARRIER_ASYNC_BLOCKING, READY_BLOCKING, "barrier_async_blocking")]
#[bench::value(&Barrier::<mode::Async>::new(1))]
fn barrier_async_blocking(barrier: &Barrier<mode::Async>) -> bool {
    black_box(barrier).wait().is_leader()
}

fn ready_blocking_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(READY_BLOCKING);
    for (name, bounded) in [("bounded", true), ("unbounded", false)] {
        group.bench_function(format!("queue_send/{name}"), |bencher| {
            bencher.iter_batched_ref(|| empty_queue(bounded), queue_send, READY_BATCH);
        });
        group.bench_function(format!("queue_recv/{name}"), |bencher| {
            bencher.iter_batched_ref(|| ready_queue(bounded), queue_recv, READY_BATCH);
        });
        group.bench_function(format!("queue_recv_timeout/{name}"), |bencher| {
            bencher.iter_batched_ref(|| ready_queue(bounded), queue_recv_timeout, READY_BATCH);
        });
    }
    group.bench_function(ONESHOT_RECV.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(ready_oneshot, oneshot_recv, READY_BATCH);
    });
    group.bench_function(ONESHOT_RECV_TIMEOUT.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(ready_oneshot, oneshot_recv_timeout, READY_BATCH);
    });
    group.bench_function(WATCH_CHANGED.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(ready_watch, watch_changed, READY_BATCH);
    });
    let sync_barrier = Barrier::new(1);
    let async_barrier = Barrier::<mode::Async>::new(1);
    group.bench_function(BARRIER_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| barrier_sync(&sync_barrier));
    });
    group.bench_function(BARRIER_ASYNC_BLOCKING.benchmark_name(), |bencher| {
        bencher.iter(|| barrier_async_blocking(&async_barrier));
    });
    group.finish();
}

#[metabench::benchmark(ARC_NEW_STD, ARC_NEW_DROP, "std")]
fn arc_new_std() -> std::sync::Arc<u64> {
    std::sync::Arc::new(black_box(7_u64))
}

#[metabench::benchmark(ARC_NEW_PERFORMABLES, ARC_NEW_DROP, "performables")]
fn arc_new_performables() -> Arc<u64> {
    Arc::new(black_box(7_u64))
}

#[metabench::benchmark(ARC_DEREF_STD, ARC_DEREF, "std")]
#[bench::value(&std::sync::Arc::new(7_u64))]
fn arc_deref_std(value: &std::sync::Arc<u64>) -> u64 {
    black_box(**value)
}

#[metabench::benchmark(ARC_DEREF_PERFORMABLES, ARC_DEREF, "performables")]
#[bench::value(&Arc::new(7_u64))]
fn arc_deref_performables(value: &Arc<u64>) -> u64 {
    black_box(**value)
}

#[metabench::benchmark(ARC_CLONE_STD, ARC_CLONE_DROP, "std")]
#[bench::value(&std::sync::Arc::new(7_u64))]
fn arc_clone_std(value: &std::sync::Arc<u64>) -> std::sync::Arc<u64> {
    black_box(std::sync::Arc::clone(value))
}

#[metabench::benchmark(ARC_CLONE_PERFORMABLES, ARC_CLONE_DROP, "performables")]
#[bench::value(&Arc::new(7_u64))]
fn arc_clone_performables(value: &Arc<u64>) -> Arc<u64> {
    black_box(Arc::clone(value))
}

#[metabench::benchmark(MUTEX_LOCK_STD, MUTEX_LOCK, "std")]
#[bench::lock(&std::sync::Mutex::new(7_u64))]
fn mutex_lock_std(lock: &std::sync::Mutex<u64>) -> u64 {
    let guard = black_box(lock).lock().unwrap_or_else(PoisonError::into_inner);
    black_box(*guard)
}

#[metabench::benchmark(MUTEX_LOCK_PERFORMABLES_SYNC, MUTEX_LOCK, "performables_sync")]
#[bench::lock(&Mutex::<u64>::new(7))]
fn mutex_lock_performables_sync(lock: &Mutex<u64>) -> u64 {
    let guard = black_box(lock).lock();
    black_box(*guard)
}

#[metabench::benchmark(MUTEX_LOCK_PERFORMABLES_ASYNC_BLOCKING, MUTEX_LOCK, "performables_async_blocking")]
#[bench::lock(&Mutex::<u64, mode::Async>::new(7))]
fn mutex_lock_performables_async_blocking(lock: &Mutex<u64, mode::Async>) -> u64 {
    let guard = black_box(lock).lock();
    black_box(*guard)
}

struct AsyncMutexInput {
    lock: Mutex<u64, mode::Async>,
    context: Context<'static>,
}

fn async_mutex_input() -> AsyncMutexInput {
    AsyncMutexInput {
        lock: Mutex::new(7),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(MUTEX_LOCK_PERFORMABLES_ASYNC, MUTEX_LOCK, "performables_async")]
#[bench::lock(&mut async_mutex_input())]
fn mutex_lock_performables_async(input: &mut AsyncMutexInput) -> u64 {
    let mut lock = pin!(input.lock.lock_async());
    let Poll::Ready(guard) = lock.as_mut().poll(&mut input.context) else {
        unreachable!("the benchmark has no competing lock holder");
    };
    black_box(*guard)
}

#[metabench::benchmark(RW_LOCK_READ_STD, RW_LOCK_READ, "std")]
#[bench::lock(&std::sync::RwLock::new(7_u64))]
fn rw_lock_read_std(lock: &std::sync::RwLock<u64>) -> u64 {
    let guard = black_box(lock).read().unwrap_or_else(PoisonError::into_inner);
    black_box(*guard)
}

#[metabench::benchmark(RW_LOCK_READ_PERFORMABLES_TRY, RW_LOCK_READ, "performables_try")]
#[bench::lock(&RwLock::<u64, mode::Async>::new(7))]
fn rw_lock_read_performables_try(lock: &RwLock<u64, mode::Async>) -> u64 {
    let guard = black_box(lock)
        .try_read()
        .unwrap_or_else(|| unreachable!("the benchmark has no competing lock holder"));
    black_box(*guard)
}

#[metabench::benchmark(RW_LOCK_READ_PERFORMABLES_SYNC, RW_LOCK_READ, "performables_sync")]
#[bench::lock(&RwLock::<u64>::new(7))]
fn rw_lock_read_performables_sync(lock: &RwLock<u64>) -> u64 {
    let guard = black_box(lock).read();
    black_box(*guard)
}

struct AsyncRwLockInput {
    lock: RwLock<u64, mode::Async>,
    context: Context<'static>,
}

fn async_rw_lock_input() -> AsyncRwLockInput {
    AsyncRwLockInput {
        lock: RwLock::new(7),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(RW_LOCK_READ_PERFORMABLES_ASYNC, RW_LOCK_READ, "performables_async")]
#[bench::lock(&mut async_rw_lock_input())]
fn rw_lock_read_performables_async(input: &mut AsyncRwLockInput) -> u64 {
    let mut lock = pin!(input.lock.read_async());
    let Poll::Ready(guard) = lock.as_mut().poll(&mut input.context) else {
        unreachable!("the benchmark has no competing lock holder");
    };
    black_box(*guard)
}

#[metabench::benchmark(RW_LOCK_WRITE_STD, RW_LOCK_WRITE, "std")]
#[bench::lock(&std::sync::RwLock::new(7_u64))]
fn rw_lock_write_std(lock: &std::sync::RwLock<u64>) -> u64 {
    let mut guard = black_box(lock).write().unwrap_or_else(PoisonError::into_inner);
    *guard = black_box(*guard);
    black_box(*guard)
}

#[metabench::benchmark(RW_LOCK_WRITE_PERFORMABLES_SYNC, RW_LOCK_WRITE, "performables_sync")]
#[bench::lock(&RwLock::<u64>::new(7))]
fn rw_lock_write_performables_sync(lock: &RwLock<u64>) -> u64 {
    let mut guard = black_box(lock).write();
    *guard = black_box(*guard);
    black_box(*guard)
}

#[metabench::benchmark(RW_LOCK_WRITE_PERFORMABLES_ASYNC, RW_LOCK_WRITE, "performables_async")]
#[bench::lock(&mut async_rw_lock_input())]
fn rw_lock_write_performables_async(input: &mut AsyncRwLockInput) -> u64 {
    let mut lock = pin!(input.lock.write_async());
    let Poll::Ready(mut guard) = lock.as_mut().poll(&mut input.context) else {
        unreachable!("the benchmark has no competing lock holder");
    };
    *guard = black_box(*guard);
    black_box(*guard)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    ready_blocking_benchmarks(criterion);
    let mut group = criterion.benchmark_group(ARC_NEW_DROP);

    group.bench_function(ARC_NEW_STD.benchmark_name(), |bencher| bencher.iter(arc_new_std));
    group.bench_function(ARC_NEW_PERFORMABLES.benchmark_name(), |bencher| {
        bencher.iter(arc_new_performables);
    });
    group.finish();

    let mut group = criterion.benchmark_group(ARC_DEREF);
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);

    group.bench_function(ARC_DEREF_STD.benchmark_name(), |bencher| {
        bencher.iter(|| arc_deref_std(&standard));
    });
    group.bench_function(ARC_DEREF_PERFORMABLES.benchmark_name(), |bencher| {
        bencher.iter(|| arc_deref_performables(&performable));
    });
    group.finish();

    let mut group = criterion.benchmark_group(ARC_CLONE_DROP);
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);

    group.bench_function(ARC_CLONE_STD.benchmark_name(), |bencher| {
        bencher.iter(|| arc_clone_std(&standard));
    });
    group.bench_function(ARC_CLONE_PERFORMABLES.benchmark_name(), |bencher| {
        bencher.iter(|| arc_clone_performables(&performable));
    });
    group.finish();

    let mut group = criterion.benchmark_group(MUTEX_LOCK);
    let standard = std::sync::Mutex::new(7_u64);
    let synchronous = Mutex::<u64>::new(7);
    let performable = Mutex::<u64, mode::Async>::new(7);
    let mut asynchronous = async_mutex_input();

    group.bench_function(MUTEX_LOCK_STD.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_std(&standard));
    });
    group.bench_function(MUTEX_LOCK_PERFORMABLES_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_performables_sync(&synchronous));
    });
    group.bench_function(MUTEX_LOCK_PERFORMABLES_ASYNC_BLOCKING.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_performables_async_blocking(&performable));
    });
    group.bench_function(MUTEX_LOCK_PERFORMABLES_ASYNC.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_performables_async(&mut asynchronous));
    });
    group.finish();

    let mut read_group = criterion.benchmark_group(RW_LOCK_READ);
    let standard = std::sync::RwLock::new(7_u64);
    let synchronous = RwLock::<u64>::new(7);
    let performable = RwLock::<u64, mode::Async>::new(7);
    let mut asynchronous = async_rw_lock_input();

    read_group.bench_function(RW_LOCK_READ_STD.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_std(&standard));
    });
    read_group.bench_function(RW_LOCK_READ_PERFORMABLES_TRY.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_performables_try(&performable));
    });
    read_group.bench_function(RW_LOCK_READ_PERFORMABLES_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_performables_sync(&synchronous));
    });
    read_group.bench_function(RW_LOCK_READ_PERFORMABLES_ASYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_performables_async(&mut asynchronous));
    });
    read_group.finish();

    let mut write_group = criterion.benchmark_group(RW_LOCK_WRITE);
    write_group.bench_function(RW_LOCK_WRITE_STD.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_std(&standard));
    });
    write_group.bench_function(RW_LOCK_WRITE_PERFORMABLES_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_performables_sync(&synchronous));
    });
    write_group.bench_function(RW_LOCK_WRITE_PERFORMABLES_ASYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_performables_async(&mut asynchronous));
    });
    write_group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        ARC_NEW_STD,
        ARC_NEW_PERFORMABLES,
        ARC_DEREF_STD,
        ARC_DEREF_PERFORMABLES,
        ARC_CLONE_STD,
        ARC_CLONE_PERFORMABLES,
        MUTEX_LOCK_STD,
        MUTEX_LOCK_PERFORMABLES_SYNC,
        MUTEX_LOCK_PERFORMABLES_ASYNC_BLOCKING,
        MUTEX_LOCK_PERFORMABLES_ASYNC,
        RW_LOCK_READ_STD,
        RW_LOCK_READ_PERFORMABLES_TRY,
        RW_LOCK_READ_PERFORMABLES_SYNC,
        RW_LOCK_READ_PERFORMABLES_ASYNC,
        RW_LOCK_WRITE_STD,
        RW_LOCK_WRITE_PERFORMABLES_SYNC,
        RW_LOCK_WRITE_PERFORMABLES_ASYNC,
        QUEUE_SEND,
        QUEUE_RECV,
        QUEUE_RECV_TIMEOUT,
        ONESHOT_RECV,
        ONESHOT_RECV_TIMEOUT,
        WATCH_CHANGED,
        BARRIER_SYNC,
        BARRIER_ASYNC_BLOCKING,
    ],
);
