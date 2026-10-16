// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::mutex::MutexGuard;
use performables::sync::{Sync, barrier::Barrier, condition::Condvar, lock::RwLock, mutex::Mutex};

// These fallbacks return a fixture-private type. An inherent async method on
// Sync takes precedence and makes the type assertions below fail to compile.
struct MissingAsyncMethod;

trait LockFallback {
    fn lock_async(&self) -> MissingAsyncMethod {
        MissingAsyncMethod
    }

    fn read_async(&self) -> MissingAsyncMethod {
        MissingAsyncMethod
    }

    fn write_async(&self) -> MissingAsyncMethod {
        MissingAsyncMethod
    }
}

impl LockFallback for Mutex<u64, Sync> {}
impl LockFallback for RwLock<u64, Sync> {}

trait CondvarFallback {
    fn wait_async(&self, _guard: MutexGuard<'_, u64, Sync>) -> MissingAsyncMethod {
        MissingAsyncMethod
    }
}

impl CondvarFallback for Condvar<Sync> {}

trait BarrierFallback {
    fn wait_async(&self) -> MissingAsyncMethod {
        MissingAsyncMethod
    }
}

impl BarrierFallback for Barrier<Sync> {}

fn main() {
    let mutex = Mutex::<u64, Sync>::new(0);
    let _: MissingAsyncMethod = mutex.lock_async();

    let lock = RwLock::<u64, Sync>::new(0);
    let _: MissingAsyncMethod = lock.read_async();
    let _: MissingAsyncMethod = lock.write_async();

    let condition = Condvar::<Sync>::new();
    let _: MissingAsyncMethod = condition.wait_async(mutex.lock());

    let barrier = Barrier::<Sync>::new(1);
    let _: MissingAsyncMethod = barrier.wait_async();
}
