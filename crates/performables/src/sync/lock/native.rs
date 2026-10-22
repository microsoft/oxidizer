// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

impl<T: ?Sized> RwLock<T, Sync> {
    pub(in crate::sync) fn read_result_native(&self) -> Result<RwLockReadGuard<'_, T>, Error<RwLockReadGuard<'_, T>>> {
        let raw = if let Some(raw) = self.raw.inner.try_read() {
            raw
        } else {
            self.read_contended_native()
        };
        self.acquired_read_native(raw)
    }

    #[cold]
    fn read_contended_native(&self) -> hal::ReadGuard<'_> {
        self.record(EventKind::RwLockReadContention);
        self.raw.inner.read()
    }

    pub(in crate::sync) fn try_read_result_native(&self) -> Result<Option<RwLockReadGuard<'_, T>>, Error<RwLockReadGuard<'_, T>>> {
        if let Some(raw) = self.raw.inner.try_read() {
            self.acquired_read_native(raw).map(Some)
        } else {
            self.record(EventKind::RwLockReadContention);
            Ok(None)
        }
    }

    fn acquired_read_native<'a>(&'a self, raw: hal::ReadGuard<'a>) -> Result<RwLockReadGuard<'a, T>, Error<RwLockReadGuard<'a, T>>> {
        let guard = RwLockReadGuard {
            lock: self,
            raw: ReadGuardSync { inner: raw },
            marker: PhantomData,
        };
        self.record(EventKind::RwLockReadAccess);
        if self.raw.inner.is_poisoned() {
            self.record(EventKind::LockPoisonObserved);
            Err(Error::poisoned(guard))
        } else {
            Ok(guard)
        }
    }

    pub(in crate::sync) fn write_result_native(&self) -> Result<RwLockWriteGuard<'_, T>, Error<RwLockWriteGuard<'_, T>>> {
        let raw = if let Some(raw) = self.raw.inner.try_write() {
            raw
        } else {
            self.write_contended_native()
        };
        self.acquired_write_native(raw)
    }

    #[cold]
    fn write_contended_native(&self) -> hal::WriteGuard<'_> {
        self.record(EventKind::RwLockWriteContention);
        self.raw.inner.write()
    }

    pub(in crate::sync) fn try_write_result_native(&self) -> Result<Option<RwLockWriteGuard<'_, T>>, Error<RwLockWriteGuard<'_, T>>> {
        if let Some(raw) = self.raw.inner.try_write() {
            self.acquired_write_native(raw).map(Some)
        } else {
            self.record(EventKind::RwLockWriteContention);
            Ok(None)
        }
    }

    fn acquired_write_native<'a>(&'a self, raw: hal::WriteGuard<'a>) -> Result<RwLockWriteGuard<'a, T>, Error<RwLockWriteGuard<'a, T>>> {
        let guard = RwLockWriteGuard {
            lock: self,
            raw: WriteGuardSync { inner: raw },
            panicking_at_acquisition: std::thread::panicking(),
            marker: PhantomData,
        };
        self.record(EventKind::RwLockWriteAccess);
        if self.raw.inner.is_poisoned() {
            self.record(EventKind::LockPoisonObserved);
            Err(Error::poisoned(guard))
        } else {
            Ok(guard)
        }
    }

    pub(in crate::sync) fn is_poisoned_native(&self) -> bool {
        self.raw.inner.is_poisoned()
    }

    pub(in crate::sync) fn clear_poison_native(&self) {
        if self.raw.inner.clear_poison() {
            self.record(EventKind::LockPoisonCleared);
        }
    }

    #[cold]
    fn release_panicking_native<'a>(&'a self, raw: &mut hal::WriteGuard<'a>) {
        let unlock = hal::WriteUnlock::new(&self.raw.inner, raw);
        if self.raw.inner.prepare_poison() {
            self.record(EventKind::LockPoisoned);
        }
        drop(unlock);
    }
}

impl<T: ?Sized> RwLockReadGuard<'_, T, Sync> {
    pub(in crate::sync) fn release_native(&mut self) {
        // The sealed destructor releases once. Native markers need no active
        // flag; the portable adapter consumes its own owning std guard.
        self.lock.raw.inner.read_unlock(&mut self.raw.inner);
        self.lock.record(EventKind::RwLockReadRelease);
    }
}

impl<T: ?Sized> RwLockWriteGuard<'_, T, Sync> {
    pub(in crate::sync) fn release_native(&mut self) {
        let lock = self.lock;
        if !self.panicking_at_acquisition && std::thread::panicking() {
            lock.release_panicking_native(&mut self.raw.inner);
        } else {
            lock.raw.inner.write_unlock(&mut self.raw.inner);
        }
        lock.record(EventKind::RwLockWriteRelease);
    }
}
