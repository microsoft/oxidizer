// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{LockResult, MutexGuard as NativeGuard, TryLockError};

use super::*;

impl<T: ?Sized> Mutex<T, Sync> {
    pub(in crate::sync) fn lock_result_native(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        match self.raw.try_lock() {
            Ok(raw) => self.acquired_native(Ok(raw)),
            Err(TryLockError::Poisoned(error)) => self.acquired_native(Err(error)),
            Err(TryLockError::WouldBlock) => {
                self.record(EventKind::MutexContention);
                self.acquired_native(self.raw.lock())
            }
        }
    }

    pub(in crate::sync) fn try_lock_result_native(&self) -> Result<Option<MutexGuard<'_, T>>, PoisonError<MutexGuard<'_, T>>> {
        match self.raw.try_lock() {
            Ok(raw) => self.acquired_native(Ok(raw)).map(Some),
            Err(TryLockError::Poisoned(error)) => self.acquired_native(Err(error)).map(Some),
            Err(TryLockError::WouldBlock) => {
                self.record(EventKind::MutexContention);
                Ok(None)
            }
        }
    }

    pub(in crate::sync) fn acquired_native<'a>(
        &'a self,
        result: LockResult<NativeGuard<'a, ()>>,
    ) -> Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>> {
        let (raw, poisoned) = match result {
            Ok(raw) => (raw, false),
            Err(error) => (error.into_inner(), true),
        };
        let guard = MutexGuard {
            mutex: self,
            raw: Some(raw),
            panicking_at_acquisition: std::thread::panicking(),
            marker: PhantomData,
        };
        self.record(EventKind::MutexAccess);
        if poisoned {
            self.record(EventKind::LockPoisonObserved);
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    pub(in crate::sync) fn is_poisoned_native(&self) -> bool {
        self.raw.is_poisoned()
    }

    pub(in crate::sync) fn clear_poison_native(&self) {
        let was_poisoned = self.raw.is_poisoned();
        self.raw.clear_poison();
        if was_poisoned {
            self.record(EventKind::LockPoisonCleared);
        }
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T, Sync> {
    pub(in crate::sync) fn into_native(mut self) -> NativeGuard<'a, ()> {
        let raw = self.raw.take().expect("a live mutex guard retains its native guard");
        // std atomically unlocks inside Condvar::wait; record logical release
        // immediately before handing ownership to that native operation.
        self.mutex.record(EventKind::MutexRelease);
        raw
    }

    pub(in crate::sync) fn release_native(&mut self) {
        if let Some(raw) = self.raw.take() {
            if !self.panicking_at_acquisition && std::thread::panicking() && !self.mutex.raw.is_poisoned() {
                self.mutex.record(EventKind::LockPoisoned);
            }
            drop(raw);
            self.mutex.record(EventKind::MutexRelease);
        }
    }
}
