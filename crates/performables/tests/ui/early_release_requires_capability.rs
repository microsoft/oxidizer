// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::{Mode, lock::RwLockReadGuard, lock::RwLockWriteGuard, mutex::MutexGuard};

fn release_mutex<M: Mode>(guard: &mut MutexGuard<'_, u64, M>) {
    M::mutex_release(guard);
}

fn release_read<M: Mode>(guard: &mut RwLockReadGuard<'_, u64, M>) {
    M::rwlock_release_read(guard);
}

fn release_write<M: Mode>(guard: &mut RwLockWriteGuard<'_, u64, M>) {
    M::rwlock_release_write(guard);
}

fn main() {}
