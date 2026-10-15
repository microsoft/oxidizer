// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

use performables::arc::{Arc, PerNuma, PerThread};
use performables::sync::{Async, barrier::Barrier, condition::Condvar, lock::RwLock, mutex::Mutex};
use thread_aware_core::ThreadAware;

fn require_thread_aware<T: ThreadAware>() {}

fn main() {
    let mutex = Mutex::<u64, Async>::new(0);
    drop(mutex.lock_async());

    let lock = RwLock::<u64, Async>::new(0);
    drop(lock.read_async());
    drop(lock.write_async());

    let condition = Condvar::<Async>::new();
    drop(condition.wait_async(mutex.lock()));

    let barrier = Barrier::<Async>::new(1);
    drop(barrier.wait_async());

    require_thread_aware::<Arc<u64, PerThread>>();
    require_thread_aware::<Arc<u64, PerNuma>>();
    let _local = Arc::<Cell<u64>, PerThread>::new_with(|| Cell::new(0));
    let _numa_local = Arc::<Cell<u64>, PerNuma>::new_with(|| Cell::new(0));
}
