// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod address;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) use address::{ReadGuard, RwLock, WriteGuard};

#[cfg(any(test, not(any(target_os = "linux", target_os = "windows"))))]
mod portable;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(super) use portable::{ReadGuard, RwLock, WriteGuard};

/// Retains exclusive ownership while poison telemetry may unwind.
pub(super) struct WriteUnlock<'lock, 'guard> {
    lock: &'lock RwLock,
    guard: &'guard mut WriteGuard<'lock>,
}

impl<'lock, 'guard> WriteUnlock<'lock, 'guard> {
    pub(super) fn new(lock: &'lock RwLock, guard: &'guard mut WriteGuard<'lock>) -> Self {
        Self { lock, guard }
    }
}

impl Drop for WriteUnlock<'_, '_> {
    fn drop(&mut self) {
        self.lock.write_unlock(self.guard);
    }
}
