// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uncontended ownership and lock benchmarks.

use std::hint::black_box;
use std::pin::pin;
use std::sync::PoisonError;
use std::task::{Context, Poll, Waker};

use criterion::{Criterion, criterion_group, criterion_main};
use performables::arc::Arc;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;

fn arc_new_drop(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_uncontended/arc_new_drop");

    group.bench_function("std", |bencher| {
        bencher.iter(|| std::sync::Arc::new(black_box(7_u64)));
    });
    group.bench_function("performables", |bencher| {
        bencher.iter(|| Arc::new(black_box(7_u64)));
    });
    group.finish();
}

fn arc_deref(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_uncontended/arc_deref");
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);

    group.bench_function("std", |bencher| bencher.iter(|| black_box(*standard)));
    group.bench_function("performables", |bencher| bencher.iter(|| black_box(*performable)));
    group.finish();
}

fn arc_clone_drop(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_uncontended/arc_clone_drop");
    let standard = std::sync::Arc::new(7_u64);
    let performable = Arc::new(7_u64);

    group.bench_function("std", |bencher| {
        bencher.iter(|| black_box(std::sync::Arc::clone(&standard)));
    });
    group.bench_function("performables", |bencher| {
        bencher.iter(|| black_box(Arc::clone(&performable)));
    });
    group.finish();
}

fn mutex_lock(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("performables_uncontended/mutex_lock");
    let standard = std::sync::Mutex::new(7_u64);
    let performable = Mutex::new(7_u64);
    let mut context = Context::from_waker(Waker::noop());

    group.bench_function("std", |bencher| {
        bencher.iter(|| {
            let guard = standard.lock().unwrap_or_else(PoisonError::into_inner);
            black_box(*guard)
        });
    });
    group.bench_function("performables", |bencher| {
        bencher.iter(|| {
            // Stack-pin to avoid allocator noise on the measured path.
            let mut lock = pin!(performable.lock());
            let Poll::Ready(guard) = lock.as_mut().poll(&mut context) else {
                unreachable!("the benchmark has no competing lock holder");
            };
            black_box(*guard)
        });
    });
    group.finish();
}

fn rw_lock(criterion: &mut Criterion) {
    let mut read_group = criterion.benchmark_group("performables_uncontended/rw_lock_read");
    let standard = std::sync::RwLock::new(7_u64);
    let performable = RwLock::new(7_u64);
    let mut context = Context::from_waker(Waker::noop());

    read_group.bench_function("std", |bencher| {
        bencher.iter(|| {
            let guard = standard.read().unwrap_or_else(PoisonError::into_inner);
            black_box(*guard)
        });
    });
    read_group.bench_function("performables_try", |bencher| {
        bencher.iter(|| {
            let guard = performable
                .try_read()
                .unwrap_or_else(|| unreachable!("the benchmark has no competing lock holder"));
            black_box(*guard)
        });
    });
    read_group.bench_function("performables_async", |bencher| {
        bencher.iter(|| {
            // Stack-pin to avoid allocator noise on the measured path.
            let mut lock = pin!(performable.read());
            let Poll::Ready(guard) = lock.as_mut().poll(&mut context) else {
                unreachable!("the benchmark has no competing lock holder");
            };
            black_box(*guard)
        });
    });
    read_group.finish();

    let mut write_group = criterion.benchmark_group("performables_uncontended/rw_lock_write");
    write_group.bench_function("std", |bencher| {
        bencher.iter(|| {
            let mut guard = standard.write().unwrap_or_else(PoisonError::into_inner);
            *guard = black_box(*guard);
            black_box(*guard)
        });
    });
    write_group.bench_function("performables", |bencher| {
        bencher.iter(|| {
            // Stack-pin to avoid allocator noise on the measured path.
            let mut lock = pin!(performable.write());
            let Poll::Ready(mut guard) = lock.as_mut().poll(&mut context) else {
                unreachable!("the benchmark has no competing lock holder");
            };
            *guard = black_box(*guard);
            black_box(*guard)
        });
    });
    write_group.finish();
}

criterion_group!(benches, arc_new_drop, arc_deref, arc_clone_drop, mutex_lock, rw_lock);
criterion_main!(benches);
