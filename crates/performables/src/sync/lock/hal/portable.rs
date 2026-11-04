// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{PoisonError, TryLockError};

// Other targets retain std's native waiting rather than an untested OS adapter.
#[derive(Debug)]
pub(in crate::sync::lock) struct RwLock {
    inner: std::sync::RwLock<()>,
}

#[derive(Debug)]
pub(in crate::sync::lock) struct ReadGuard<'a> {
    inner: Option<std::sync::RwLockReadGuard<'a, ()>>,
}

#[derive(Debug)]
pub(in crate::sync::lock) struct WriteGuard<'a> {
    inner: Option<std::sync::RwLockWriteGuard<'a, ()>>,
}

impl RwLock {
    pub(in crate::sync::lock) const fn new() -> Self {
        Self {
            inner: std::sync::RwLock::new(()),
        }
    }

    pub(in crate::sync::lock) fn try_read(&self) -> Option<ReadGuard<'_>> {
        let guard = match self.inner.try_read() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(ReadGuard { inner: Some(guard) })
    }

    pub(in crate::sync::lock) fn read(&self) -> ReadGuard<'_> {
        ReadGuard {
            inner: Some(self.inner.read().unwrap_or_else(PoisonError::into_inner)),
        }
    }

    pub(in crate::sync::lock) fn try_write(&self) -> Option<WriteGuard<'_>> {
        let guard = match self.inner.try_write() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(WriteGuard { inner: Some(guard) })
    }

    pub(in crate::sync::lock) fn write(&self) -> WriteGuard<'_> {
        WriteGuard {
            inner: Some(self.inner.write().unwrap_or_else(PoisonError::into_inner)),
        }
    }

    #[expect(clippy::unused_self, reason = "the portable owning guard retains its native lock")]
    pub(in crate::sync::lock) fn read_unlock(&self, guard: &mut ReadGuard<'_>) {
        drop(guard.inner.take());
    }

    #[expect(clippy::unused_self, reason = "the portable owning guard retains its native lock")]
    pub(in crate::sync::lock) fn write_unlock(&self, guard: &mut WriteGuard<'_>) {
        drop(guard.inner.take());
    }

    pub(in crate::sync::lock) fn is_poisoned(&self) -> bool {
        self.inner.is_poisoned()
    }

    pub(in crate::sync::lock) fn clear_poison(&self) -> bool {
        let previous = self.inner.is_poisoned();
        self.inner.clear_poison();
        previous
    }

    pub(in crate::sync::lock) fn prepare_poison(&self) -> bool {
        !self.inner.is_poisoned()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    #[test]
    fn fallback_preserves_ownership_and_poison_recovery() {
        let lock = RwLock::new();
        let mut reader = lock.read();
        let mut second = lock.try_read().unwrap();
        let writer_blocked = lock.try_write().is_none();
        lock.read_unlock(&mut reader);
        lock.read_unlock(&mut second);
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _writer = lock.write();
            assert!(lock.prepare_poison());
            assert!(lock.try_read().is_none());
            panic!("poison the fallback writer");
        }))
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>().copied(), Some("poison the fallback writer"));
        let mut recovered = lock.try_write().unwrap();
        assert!(lock.is_poisoned());
        assert!(!lock.prepare_poison());
        lock.write_unlock(&mut recovered);
        let mut recovered = lock.try_read().unwrap();
        lock.read_unlock(&mut recovered);
        let mut recovered = lock.write();
        lock.write_unlock(&mut recovered);
        let cleared = lock.clear_poison();
        let mut writer = lock.try_write().unwrap();
        lock.write_unlock(&mut writer);
        assert_eq!(
            (writer_blocked, cleared, lock.clear_poison(), lock.is_poisoned()),
            (true, true, false, false)
        );
    }
}
