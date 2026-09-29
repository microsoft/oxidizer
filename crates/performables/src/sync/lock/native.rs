// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{LockResult, RwLockReadGuard as NativeReadGuard, RwLockWriteGuard as NativeWriteGuard, TryLockError, TryLockResult};

use super::*;

impl<T: ?Sized> RwLock<T, Sync> {
    pub(in crate::sync) fn read_result_native(&self) -> Result<RwLockReadGuard<'_, T>, PoisonError<RwLockReadGuard<'_, T>>> {
        self.read_result_from_native(self.raw.try_read())
    }

    pub(in crate::sync) fn read_result_from_native<'a>(
        &'a self,
        result: TryLockResult<NativeReadGuard<'a, ()>>,
    ) -> Result<RwLockReadGuard<'a, T>, PoisonError<RwLockReadGuard<'a, T>>> {
        match result {
            Ok(raw) => self.acquired_read_native(Ok(raw)),
            Err(TryLockError::Poisoned(error)) => self.acquired_read_native(Err(error)),
            Err(TryLockError::WouldBlock) => self.wait_for_read_native(),
        }
    }

    pub(in crate::sync) fn wait_for_read_native(&self) -> Result<RwLockReadGuard<'_, T>, PoisonError<RwLockReadGuard<'_, T>>> {
        self.record(EventKind::RwLockReadContention);
        self.acquired_read_native(self.raw.read())
    }

    pub(in crate::sync) fn try_read_result_native(&self) -> Result<Option<RwLockReadGuard<'_, T>>, PoisonError<RwLockReadGuard<'_, T>>> {
        match self.raw.try_read() {
            Ok(raw) => self.acquired_read_native(Ok(raw)).map(Some),
            Err(TryLockError::Poisoned(error)) => self.acquired_read_native(Err(error)).map(Some),
            Err(TryLockError::WouldBlock) => {
                self.record(EventKind::RwLockReadContention);
                Ok(None)
            }
        }
    }

    pub(in crate::sync) fn write_result_native(&self) -> Result<RwLockWriteGuard<'_, T>, PoisonError<RwLockWriteGuard<'_, T>>> {
        self.write_result_from_native(self.raw.try_write())
    }

    pub(in crate::sync) fn write_result_from_native<'a>(
        &'a self,
        result: TryLockResult<NativeWriteGuard<'a, ()>>,
    ) -> Result<RwLockWriteGuard<'a, T>, PoisonError<RwLockWriteGuard<'a, T>>> {
        match result {
            Ok(raw) => self.acquired_write_native(Ok(raw)),
            Err(TryLockError::Poisoned(error)) => self.acquired_write_native(Err(error)),
            Err(TryLockError::WouldBlock) => {
                self.record(EventKind::RwLockWriteContention);
                self.acquired_write_native(self.raw.write())
            }
        }
    }

    pub(in crate::sync) fn try_write_result_native(&self) -> Result<Option<RwLockWriteGuard<'_, T>>, PoisonError<RwLockWriteGuard<'_, T>>> {
        match self.raw.try_write() {
            Ok(raw) => self.acquired_write_native(Ok(raw)).map(Some),
            Err(TryLockError::Poisoned(error)) => self.acquired_write_native(Err(error)).map(Some),
            Err(TryLockError::WouldBlock) => {
                self.record(EventKind::RwLockWriteContention);
                Ok(None)
            }
        }
    }

    fn acquired_read_native<'a>(
        &'a self,
        result: LockResult<NativeReadGuard<'a, ()>>,
    ) -> Result<RwLockReadGuard<'a, T>, PoisonError<RwLockReadGuard<'a, T>>> {
        let (raw, poisoned) = match result {
            Ok(raw) => (raw, false),
            Err(error) => (error.into_inner(), true),
        };
        let guard = RwLockReadGuard {
            lock: self,
            raw: Some(raw),
            marker: PhantomData,
        };
        self.record(EventKind::RwLockReadAccess);
        if poisoned {
            self.record(EventKind::LockPoisonObserved);
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    fn acquired_write_native<'a>(
        &'a self,
        result: LockResult<NativeWriteGuard<'a, ()>>,
    ) -> Result<RwLockWriteGuard<'a, T>, PoisonError<RwLockWriteGuard<'a, T>>> {
        let (raw, poisoned) = match result {
            Ok(raw) => (raw, false),
            Err(error) => (error.into_inner(), true),
        };
        let guard = RwLockWriteGuard {
            lock: self,
            raw: Some(raw),
            panicking_at_acquisition: std::thread::panicking(),
            marker: PhantomData,
        };
        self.record(EventKind::RwLockWriteAccess);
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

impl<T: ?Sized> RwLockReadGuard<'_, T, Sync> {
    pub(in crate::sync) fn release_native(&mut self) {
        if let Some(raw) = self.raw.take() {
            drop(raw);
            self.lock.record(EventKind::RwLockReadRelease);
        }
    }
}

impl<T: ?Sized> RwLockWriteGuard<'_, T, Sync> {
    pub(in crate::sync) fn release_native(&mut self) {
        if let Some(raw) = self.raw.take() {
            if !self.panicking_at_acquisition && std::thread::panicking() && !self.lock.raw.is_poisoned() {
                self.lock.record(EventKind::LockPoisoned);
            }
            drop(raw);
            self.lock.record(EventKind::RwLockWriteRelease);
        }
    }
}
