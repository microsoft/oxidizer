// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uncontended ownership and lock benchmarks.

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

use criterion::Criterion;
use performables::arc::Arc;
use performables::sync::lock::RwLock;
use performables::sync::mode;
use performables::sync::mutex::Mutex;

const ARC_NEW_DROP: &str = "performables_uncontended/arc_new_drop";
const ARC_DEREF: &str = "performables_uncontended/arc_deref";
const ARC_CLONE_DROP: &str = "performables_uncontended/arc_clone_drop";
const MUTEX_LOCK: &str = "performables_uncontended/mutex_lock";
const RW_LOCK_READ: &str = "performables_uncontended/rw_lock_read";
const RW_LOCK_WRITE: &str = "performables_uncontended/rw_lock_write";

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
    ],
);
