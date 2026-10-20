// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints target-specific inline sizes and empty watch construction allocations.

use alloc_tracker::{Allocator, Session};
use performables::arc::{Arc, PerNuma, PerThread, Weak};
use performables::sync;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{OneshotReceiver, OneshotSender, Receiver, Sender, WatchReceiver, WatchRef, WatchSender};
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::notify::Notify;
use performables::sync::once::{LazyLock, OnceLock};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("Inline bytes on this target; payload T = u64");
    println!("{:<24} {:>8} {:>8}", "Type", "Sync", "Async");
    modes::<Mutex<u64>, Mutex<u64, sync::Async>>("Mutex");
    modes::<RwLock<u64>, RwLock<u64, sync::Async>>("RwLock");
    modes::<Condvar, Condvar<sync::Async>>("Condvar");
    modes::<Barrier, Barrier<sync::Async>>("Barrier");
    modes::<Notify, Notify<sync::Async>>("Notify");
    println!();
    fixed::<Arc<u64>>("Arc");
    fixed::<Weak<u64>>("Weak");
    fixed::<Arc<u64, PerThread>>("Arc<_, PerThread>");
    fixed::<Arc<u64, PerNuma>>("Arc<_, PerNuma>");
    fixed::<OnceLock<u64>>("OnceLock");
    fixed::<LazyLock<u64>>("LazyLock");
    fixed::<Sender<u64>>("Sender");
    fixed::<Receiver<u64>>("Receiver");
    fixed::<OneshotSender<u64>>("OneshotSender");
    fixed::<OneshotReceiver<u64>>("OneshotReceiver");
    fixed::<WatchSender<u64>>("WatchSender");
    fixed::<WatchReceiver<u64>>("WatchReceiver");
    fixed::<WatchRef<'_, u64>>("WatchRef");
    fixed::<std::sync::MutexGuard<'_, u64>>("std::MutexGuard");
    fixed::<std::sync::Mutex<u64>>("std::Mutex");
    fixed::<std::sync::RwLock<u64>>("std::RwLock");
    fixed::<std::sync::Condvar>("std::Condvar");
    fixed::<std::sync::Barrier>("std::Barrier");
    println!("\nEmpty watch<bool>: construction heap requests, not RSS or allocator size classes");
    println!("{:<24} {:>8} {:>8}", "Implementation", "Allocs", "Bytes");
    construction("performables", || performables::sync::channel::watch(false));
    construction("tokio", || tokio::sync::watch::channel(false));
    println!("std: no native watch primitive");
}

fn modes<S, A>(name: &str) {
    println!("{name:<24} {:>8} {:>8}", size_of::<S>(), size_of::<A>());
}

fn fixed<T>(name: &str) {
    println!("{name:<24} {:>8}", size_of::<T>());
}

fn construction<T>(name: &str, create: impl FnOnce() -> T) {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("watch_construction");
    let value = {
        let _measurement = operation.measure_thread().iterations(1);
        std::hint::black_box(create())
    };
    let report = session.to_report();
    let statistics = report
        .operations()
        .next()
        .expect("the measured watch construction produces one report entry")
        .1;
    println!(
        "{name:<24} {:>8} {:>8}",
        statistics.total_allocations_count(),
        statistics.total_bytes_allocated()
    );
    drop(value);
}
