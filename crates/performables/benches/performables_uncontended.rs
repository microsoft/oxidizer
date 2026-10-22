// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uncontended ownership and lock benchmarks.
//!
//! With `seismograph` enabled, recording stays disabled. The configuration
//! crosses an optimization barrier so runtime-enabled recording remains
//! reachable in the compiled workloads.
//! Both lock implementations panic on poison.
//! Tokio cases measure already-ready futures without entering an executor.
//! Tokio guards do not poison and have different contention/fairness guarantees.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::hint::black_box;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use criterion::Criterion;
use performables::arc::Arc;
use performables::sync;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
#[cfg(feature = "seismograph")]
use seismograph::recorder::Configuration;

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
#[bench::value(Box::new(std::sync::Arc::new(7_u64)))]
fn arc_deref_std<S: std::borrow::Borrow<std::sync::Arc<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(**value)
    };
    (result, owned_input)
}

#[metabench::benchmark(ARC_DEREF_PERFORMABLES, ARC_DEREF, "performables")]
#[bench::value(Box::new(Arc::new(7_u64)))]
fn arc_deref_performables<S: std::borrow::Borrow<Arc<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(**value)
    };
    (result, owned_input)
}

#[metabench::benchmark(ARC_CLONE_STD, ARC_CLONE_DROP, "std")]
#[bench::value(Box::new(std::sync::Arc::new(7_u64)))]
fn arc_clone_std<S: std::borrow::Borrow<std::sync::Arc<u64>>>(owned_input: S) -> (std::sync::Arc<u64>, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(std::sync::Arc::clone(value))
    };
    (result, owned_input)
}

#[metabench::benchmark(ARC_CLONE_PERFORMABLES, ARC_CLONE_DROP, "performables")]
#[bench::value(Box::new(Arc::new(7_u64)))]
fn arc_clone_performables<S: std::borrow::Borrow<Arc<u64>>>(owned_input: S) -> (Arc<u64>, S) {
    let result = {
        let value = owned_input.borrow();
        black_box(Arc::clone(value))
    };
    (result, owned_input)
}

#[metabench::benchmark(MUTEX_LOCK_STD, MUTEX_LOCK, "std")]
#[bench::lock(Box::new(std::sync::Mutex::new(7_u64)))]
fn mutex_lock_std<S: std::borrow::Borrow<std::sync::Mutex<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let guard = black_box(lock).lock().expect("the benchmark has no panicking lock holder");
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(MUTEX_LOCK_PERFORMABLES_SYNC, MUTEX_LOCK, "performables_sync")]
#[bench::lock(Box::new(Mutex::<u64>::new(7)))]
fn mutex_lock_performables_sync<S: std::borrow::Borrow<Mutex<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let guard = black_box(lock).lock();
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(MUTEX_LOCK_PERFORMABLES_ASYNC_BLOCKING, MUTEX_LOCK, "performables_async_blocking")]
#[bench::lock(Box::new(Mutex::<u64, sync::Async>::new(7)))]
fn mutex_lock_performables_async_blocking<S: std::borrow::Borrow<Mutex<u64, sync::Async>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let guard = black_box(lock).lock();
        black_box(*guard)
    };
    (result, owned_input)
}

struct AsyncMutexInput {
    lock: Mutex<u64, sync::Async>,
    context: Context<'static>,
}

fn async_mutex_input() -> AsyncMutexInput {
    AsyncMutexInput {
        lock: Mutex::new(7),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(MUTEX_LOCK_PERFORMABLES_ASYNC, MUTEX_LOCK, "performables_async")]
#[bench::lock(Box::new(async_mutex_input()))]
fn mutex_lock_performables_async<S: std::borrow::BorrowMut<AsyncMutexInput>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let mut lock = pin!(input.lock.lock_async());
        let Poll::Ready(guard) = lock.as_mut().poll(&mut input.context) else {
            unreachable!("the benchmark has no competing lock holder");
        };
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(MUTEX_LOCK_TOKIO, MUTEX_LOCK, "tokio")]
#[bench::lock(&mut tokio_mutex_input())]
fn mutex_lock_tokio(input: &mut TokioMutexInput) -> u64 {
    // Stack-pin to avoid allocator noise on the measured path.
    let mut future = pin!(input.lock.lock());
    let Poll::Ready(guard) = future.as_mut().poll(&mut input.context) else {
        unreachable!("the benchmark has no competing lock holder");
    };
    black_box(*guard)
}

struct TokioMutexInput {
    lock: tokio::sync::Mutex<u64>,
    context: Context<'static>,
}

fn tokio_mutex_input() -> TokioMutexInput {
    TokioMutexInput {
        lock: tokio::sync::Mutex::new(7),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(RW_LOCK_READ_STD, RW_LOCK_READ, "std")]
#[bench::lock(Box::new(std::sync::RwLock::new(7_u64)))]
fn rw_lock_read_std<S: std::borrow::Borrow<std::sync::RwLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let guard = black_box(lock).read().expect("the benchmark has no panicking lock holder");
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(RW_LOCK_READ_PERFORMABLES_TRY, RW_LOCK_READ, "performables_try")]
#[bench::lock(Box::new(RwLock::<u64, sync::Async>::new(7)))]
fn rw_lock_read_performables_try<S: std::borrow::Borrow<RwLock<u64, sync::Async>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let guard = black_box(lock)
            .try_read()
            .unwrap_or_else(|| unreachable!("the benchmark has no competing lock holder"));
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(RW_LOCK_READ_PERFORMABLES_SYNC, RW_LOCK_READ, "performables_sync")]
#[bench::lock(Box::new(RwLock::<u64>::new(7)))]
fn rw_lock_read_performables_sync<S: std::borrow::Borrow<RwLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let guard = black_box(lock).read();
        black_box(*guard)
    };
    (result, owned_input)
}

struct AsyncRwLockInput {
    lock: RwLock<u64, sync::Async>,
    context: Context<'static>,
}

fn async_rw_lock_input() -> AsyncRwLockInput {
    AsyncRwLockInput {
        lock: RwLock::new(7),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(RW_LOCK_READ_PERFORMABLES_ASYNC, RW_LOCK_READ, "performables_async")]
#[bench::lock(Box::new(async_rw_lock_input()))]
fn rw_lock_read_performables_async<S: std::borrow::BorrowMut<AsyncRwLockInput>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let mut lock = pin!(input.lock.read_async());
        let Poll::Ready(guard) = lock.as_mut().poll(&mut input.context) else {
            unreachable!("the benchmark has no competing lock holder");
        };
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(RW_LOCK_READ_TOKIO, RW_LOCK_READ, "tokio")]
#[bench::lock(&mut tokio_rwlock_input())]
fn rw_lock_read_tokio(input: &mut TokioRwLockInput) -> u64 {
    // Stack-pin to avoid allocator noise on the measured path.
    let mut future = pin!(input.lock.read());
    let Poll::Ready(guard) = future.as_mut().poll(&mut input.context) else {
        unreachable!("the benchmark has no competing lock holder");
    };
    black_box(*guard)
}

struct TokioRwLockInput {
    lock: tokio::sync::RwLock<u64>,
    context: Context<'static>,
}

fn tokio_rwlock_input() -> TokioRwLockInput {
    TokioRwLockInput {
        lock: tokio::sync::RwLock::new(7),
        context: Context::from_waker(Waker::noop()),
    }
}

#[metabench::benchmark(RW_LOCK_WRITE_STD, RW_LOCK_WRITE, "std")]
#[bench::lock(Box::new(std::sync::RwLock::new(7_u64)))]
fn rw_lock_write_std<S: std::borrow::Borrow<std::sync::RwLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let mut guard = black_box(lock).write().expect("the benchmark has no panicking lock holder");
        *guard = black_box(*guard);
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(RW_LOCK_WRITE_PERFORMABLES_SYNC, RW_LOCK_WRITE, "performables_sync")]
#[bench::lock(Box::new(RwLock::<u64>::new(7)))]
fn rw_lock_write_performables_sync<S: std::borrow::Borrow<RwLock<u64>>>(owned_input: S) -> (u64, S) {
    let result = {
        let lock = owned_input.borrow();
        let mut guard = black_box(lock).write();
        *guard = black_box(*guard);
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(RW_LOCK_WRITE_PERFORMABLES_ASYNC, RW_LOCK_WRITE, "performables_async")]
#[bench::lock(Box::new(async_rw_lock_input()))]
fn rw_lock_write_performables_async<S: std::borrow::BorrowMut<AsyncRwLockInput>>(mut owned_input: S) -> (u64, S) {
    let result = {
        let input = owned_input.borrow_mut();
        let mut lock = pin!(input.lock.write_async());
        let Poll::Ready(mut guard) = lock.as_mut().poll(&mut input.context) else {
            unreachable!("the benchmark has no competing lock holder");
        };
        *guard = black_box(*guard);
        black_box(*guard)
    };
    (result, owned_input)
}

#[metabench::benchmark(RW_LOCK_WRITE_TOKIO, RW_LOCK_WRITE, "tokio")]
#[bench::lock(&mut tokio_rwlock_input())]
fn rw_lock_write_tokio(input: &mut TokioRwLockInput) -> u64 {
    // Stack-pin to avoid allocator noise on the measured path.
    let mut future = pin!(input.lock.write());
    let Poll::Ready(mut guard) = future.as_mut().poll(&mut input.context) else {
        unreachable!("the benchmark has no competing lock holder");
    };
    *guard = black_box(*guard);
    black_box(*guard)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    #[cfg(feature = "seismograph")]
    seismograph::recorder(black_box(Configuration::default()));

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
        bencher.iter(|| arc_deref_std(&standard).0);
    });
    group.bench_function(ARC_DEREF_PERFORMABLES.benchmark_name(), |bencher| {
        bencher.iter(|| arc_deref_performables(&performable).0);
    });
    group.finish();

    let mut group = criterion.benchmark_group(ARC_CLONE_DROP);
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);

    group.bench_function(ARC_CLONE_STD.benchmark_name(), |bencher| {
        bencher.iter(|| arc_clone_std(&standard).0);
    });
    group.bench_function(ARC_CLONE_PERFORMABLES.benchmark_name(), |bencher| {
        bencher.iter(|| arc_clone_performables(&performable).0);
    });
    group.finish();

    let mut group = criterion.benchmark_group(MUTEX_LOCK);
    let standard = std::sync::Mutex::new(7_u64);
    let synchronous = Mutex::<u64>::new(7);
    let performable = Mutex::<u64, sync::Async>::new(7);
    let mut asynchronous = async_mutex_input();

    group.bench_function(MUTEX_LOCK_STD.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_std(&standard).0);
    });
    group.bench_function(MUTEX_LOCK_PERFORMABLES_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_performables_sync(&synchronous).0);
    });
    group.bench_function(MUTEX_LOCK_PERFORMABLES_ASYNC_BLOCKING.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_performables_async_blocking(&performable).0);
    });
    group.bench_function(MUTEX_LOCK_PERFORMABLES_ASYNC.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_performables_async(&mut asynchronous).0);
    });
    let mut tokio = tokio_mutex_input();
    group.bench_function(MUTEX_LOCK_TOKIO.benchmark_name(), |bencher| {
        bencher.iter(|| mutex_lock_tokio(&mut tokio));
    });
    group.finish();

    let mut read_group = criterion.benchmark_group(RW_LOCK_READ);
    let standard = std::sync::RwLock::new(7_u64);
    let synchronous = RwLock::<u64>::new(7);
    let performable = RwLock::<u64, sync::Async>::new(7);
    let mut asynchronous = async_rw_lock_input();

    read_group.bench_function(RW_LOCK_READ_STD.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_std(&standard).0);
    });
    read_group.bench_function(RW_LOCK_READ_PERFORMABLES_TRY.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_performables_try(&performable).0);
    });
    read_group.bench_function(RW_LOCK_READ_PERFORMABLES_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_performables_sync(&synchronous).0);
    });
    read_group.bench_function(RW_LOCK_READ_PERFORMABLES_ASYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_performables_async(&mut asynchronous).0);
    });
    let mut tokio = tokio_rwlock_input();
    read_group.bench_function(RW_LOCK_READ_TOKIO.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_read_tokio(&mut tokio));
    });
    read_group.finish();

    let mut write_group = criterion.benchmark_group(RW_LOCK_WRITE);
    write_group.bench_function(RW_LOCK_WRITE_STD.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_std(&standard).0);
    });
    write_group.bench_function(RW_LOCK_WRITE_PERFORMABLES_SYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_performables_sync(&synchronous).0);
    });
    write_group.bench_function(RW_LOCK_WRITE_PERFORMABLES_ASYNC.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_performables_async(&mut asynchronous).0);
    });
    write_group.bench_function(RW_LOCK_WRITE_TOKIO.benchmark_name(), |bencher| {
        bencher.iter(|| rw_lock_write_tokio(&mut tokio));
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
        MUTEX_LOCK_TOKIO,
        RW_LOCK_READ_STD,
        RW_LOCK_READ_PERFORMABLES_TRY,
        RW_LOCK_READ_PERFORMABLES_SYNC,
        RW_LOCK_READ_PERFORMABLES_ASYNC,
        RW_LOCK_READ_TOKIO,
        RW_LOCK_WRITE_STD,
        RW_LOCK_WRITE_PERFORMABLES_SYNC,
        RW_LOCK_WRITE_PERFORMABLES_ASYNC,
        RW_LOCK_WRITE_TOKIO,
    ],
);
