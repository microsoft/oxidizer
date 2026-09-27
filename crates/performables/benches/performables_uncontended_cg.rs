// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic uncontended lock overhead, paired with `performables_uncontended`.
//! These cases never park or allocate waiter storage; they do not model contention.

#![allow(missing_docs, reason = "benchmark code")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::pin::pin;
    use std::sync::PoisonError;
    use std::task::{Context, Poll, Waker};

    use gungraun::{library_benchmark, library_benchmark_group};
    use performables::sync::lock::RwLock;
    use performables::sync::mode;
    use performables::sync::mutex::Mutex;

    #[library_benchmark]
    #[bench::run(&std::sync::Mutex::new(7))]
    fn mutex_lock_std(lock: &std::sync::Mutex<u64>) -> u64 {
        let guard = black_box(lock).lock().unwrap_or_else(PoisonError::into_inner);
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&Mutex::<u64>::new(7))]
    fn mutex_lock_performables_sync(lock: &Mutex<u64>) -> u64 {
        let guard = black_box(lock).lock();
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&Mutex::<u64, mode::Async>::new(7))]
    fn mutex_lock_performables_async_blocking(lock: &Mutex<u64, mode::Async>) -> u64 {
        let guard = black_box(lock).lock();
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&Mutex::<u64, mode::Async>::new(7))]
    fn mutex_lock_performables_async(lock: &Mutex<u64, mode::Async>) -> u64 {
        let guard = ready(black_box(lock).lock_async());
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&std::sync::RwLock::new(7))]
    fn rw_lock_read_std(lock: &std::sync::RwLock<u64>) -> u64 {
        let guard = black_box(lock).read().unwrap_or_else(PoisonError::into_inner);
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&RwLock::<u64>::new(7))]
    fn rw_lock_read_performables_sync(lock: &RwLock<u64>) -> u64 {
        let guard = black_box(lock).read();
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&RwLock::<u64, mode::Async>::new(7))]
    fn rw_lock_read_performables_async(lock: &RwLock<u64, mode::Async>) -> u64 {
        let guard = ready(black_box(lock).read_async());
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&std::sync::RwLock::new(7))]
    fn rw_lock_write_std(lock: &std::sync::RwLock<u64>) -> u64 {
        let mut guard = black_box(lock).write().unwrap_or_else(PoisonError::into_inner);
        *guard = black_box(*guard);
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&RwLock::<u64>::new(7))]
    fn rw_lock_write_performables_sync(lock: &RwLock<u64>) -> u64 {
        let mut guard = black_box(lock).write();
        *guard = black_box(*guard);
        black_box(*guard)
    }

    #[library_benchmark]
    #[bench::run(&RwLock::<u64, mode::Async>::new(7))]
    fn rw_lock_write_performables_async(lock: &RwLock<u64, mode::Async>) -> u64 {
        let mut guard = ready(black_box(lock).write_async());
        *guard = black_box(*guard);
        black_box(*guard)
    }

    fn ready<F: Future>(future: F) -> F::Output {
        // Stack-pin to keep allocator overhead out of the measured operation.
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => unreachable!("the benchmark has no competing lock holder"),
        }
    }

    library_benchmark_group!(
        name = mutex_lock;
        benchmarks = mutex_lock_std, mutex_lock_performables_sync,
            mutex_lock_performables_async_blocking, mutex_lock_performables_async
    );
    library_benchmark_group!(
        name = rw_lock_read;
        benchmarks = rw_lock_read_std, rw_lock_read_performables_sync, rw_lock_read_performables_async
    );
    library_benchmark_group!(
        name = rw_lock_write;
        benchmarks = rw_lock_write_std, rw_lock_write_performables_sync, rw_lock_write_performables_async
    );
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = mutex_lock, rw_lock_read, rw_lock_write);
