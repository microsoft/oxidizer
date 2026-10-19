// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints inline storage sizes; shared allocations and queue buffers are excluded.

use performables::arc::{Arc, PerNuma, PerThread, Weak};
use performables::sync;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{OneshotReceiver, OneshotSender, Receiver, Sender, WatchReceiver, WatchRef, WatchSender};
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::notify::Notify;
use performables::sync::once::{LazyLock, OnceLock};

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
}

fn modes<S, A>(name: &str) {
    println!("{name:<24} {:>8} {:>8}", size_of::<S>(), size_of::<A>());
}

fn fixed<T>(name: &str) {
    println!("{name:<24} {:>8}", size_of::<T>());
}
