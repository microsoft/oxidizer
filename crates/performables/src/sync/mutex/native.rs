// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

impl<T: ?Sized> Mutex<T, Sync> {
    pub(in crate::sync) fn native_state(&self) -> &hal::Mutex {
        &self.raw.inner
    }

    pub(in crate::sync) fn lock_result_native(&self) -> Result<MutexGuard<'_, T>, Error<MutexGuard<'_, T>>> {
        let raw = if let Some(raw) = self.raw.inner.try_lock() {
            raw
        } else {
            self.record(EventKind::MutexContention);
            self.raw.inner.lock()
        };
        self.acquired_native(raw)
    }

    pub(in crate::sync) fn try_lock_result_native(&self) -> Result<Option<MutexGuard<'_, T>>, Error<MutexGuard<'_, T>>> {
        if let Some(raw) = self.raw.inner.try_lock() {
            self.acquired_native(raw).map(Some)
        } else {
            self.record(EventKind::MutexContention);
            Ok(None)
        }
    }

    pub(in crate::sync) fn acquired_native<'a>(&'a self, raw: hal::Guard<'a>) -> Result<MutexGuard<'a, T>, Error<MutexGuard<'a, T>>> {
        let guard = MutexGuard {
            mutex: self,
            raw: GuardSync {
                inner: ManuallyDrop::new(raw),
            },
            panicking_at_acquisition: std::thread::panicking(),
            marker: PhantomData,
        };
        self.record(EventKind::MutexAccess);
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
    fn release_panicking_native<'a>(&'a self, raw: hal::Guard<'a>) {
        let unlock = hal::Unlock::new(&self.raw.inner, raw);
        if self.raw.inner.prepare_poison() {
            self.record(EventKind::LockPoisoned);
        }
        drop(unlock);
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T, Sync> {
    pub(in crate::sync) fn into_native(self) -> hal::Guard<'a> {
        let mut this = ManuallyDrop::new(self);
        let mutex = this.mutex;
        // SAFETY: consuming self suppresses its destructor, and transfers this
        // field exactly once to the wait operation's ownership evidence.
        let raw = unsafe { ManuallyDrop::take(&mut this.raw.inner) };
        let unlock = hal::Unlock::new(&mutex.raw.inner, raw);
        // The HAL orders unlock against notification after capturing the wait
        // generation. Preserve logical release before handing it ownership.
        mutex.record(EventKind::MutexRelease);
        unlock.into_guard()
    }

    pub(in crate::sync) fn release_native(&mut self) {
        let mutex = self.mutex;
        // SAFETY: only the sealed mode calls this from the guard destructor.
        // The manually dropped field is released once and is never reused.
        let raw = unsafe { ManuallyDrop::take(&mut self.raw.inner) };
        if !self.panicking_at_acquisition && std::thread::panicking() {
            mutex.release_panicking_native(raw);
        } else {
            mutex.raw.inner.unlock(raw);
        }
        mutex.record(EventKind::MutexRelease);
    }
}
