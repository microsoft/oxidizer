// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::lock::{RwLockReadGuard, RwLockWriteGuard};
use performables::sync::mutex::MutexGuard;
use performables::sync::{Async, Mode, Sync};

fn require_release_capability<M: Mode>() {
    // Infer the private capability type while requiring a second argument.
    let _: fn(&mut MutexGuard<'_, u64, M>, _) = M::mutex_release::<u64>;
    let _: fn(&mut RwLockReadGuard<'_, u64, M>, _) = M::rwlock_release_read::<u64>;
    let _: fn(&mut RwLockWriteGuard<'_, u64, M>, _) = M::rwlock_release_write::<u64>;
}

fn main() {
    require_release_capability::<Sync>();
    require_release_capability::<Async>();
}
