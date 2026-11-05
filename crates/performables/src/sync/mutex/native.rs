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
            self.lock_contended_native()
        };
        self.acquired_native(raw, std::thread::panicking())
    }

    fn lock_contended_native(&self) -> hal::Guard<'_> {
        self.record(EventKind::MutexContention);
        self.raw.inner.lock()
    }

    pub(in crate::sync) fn try_lock_result_native(&self) -> Result<Option<MutexGuard<'_, T>>, Error<MutexGuard<'_, T>>> {
        if let Some(raw) = self.raw.inner.try_lock() {
            self.acquired_native(raw, std::thread::panicking()).map(Some)
        } else {
            self.record(EventKind::MutexContention);
            Ok(None)
        }
    }

    pub(in crate::sync) fn acquired_native<'a>(
        &'a self,
        raw: hal::Guard<'a>,
        panicking_at_acquisition: bool,
    ) -> Result<MutexGuard<'a, T>, Error<MutexGuard<'a, T>>> {
        let guard = MutexGuard {
            mutex: self,
            raw: GuardSync {
                inner: ManuallyDrop::new(raw),
            },
            panicking_at_acquisition,
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
    pub(in crate::sync) fn into_native(self) -> (hal::Guard<'a>, bool) {
        let mut this = ManuallyDrop::new(self);
        let mutex = this.mutex;
        // A condition wait must retain the original guard's unwind baseline.
        let panicking_at_acquisition = this.panicking_at_acquisition;
        // SAFETY: consuming self suppresses its destructor, and transfers this
        // field exactly once to the wait operation's ownership evidence.
        let raw = unsafe { ManuallyDrop::take(&mut this.raw.inner) };
        let unlock = hal::Unlock::new(&mutex.raw.inner, raw);
        // The HAL orders unlock against notification after capturing the wait
        // generation. Preserve logical release before handing it ownership.
        mutex.record(EventKind::MutexRelease);
        (unlock.into_guard(), panicking_at_acquisition)
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

#[cfg(all(test, feature = "seismograph"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod gamma_round2_tests {
    use seismograph::recorder::event::EventKind as RecordedKind;

    use super::*;
    use crate::gamma_round2_telemetry::recorded_test_events;

    #[test]
    fn gamma_round2_native_mutex_cold_acquisition_records_the_already_detected_conflict() {
        crate::gamma_round2_telemetry::enable_recording();
        let mutex = Mutex::new(43);
        let held = mutex.lock();
        assert!(mutex.try_lock().is_none());
        drop(held);
        let before = recorded_test_events(RecordedKind::MutexContention);
        let raw = mutex.lock_contended_native();
        let guard = mutex.acquired_native(raw, false).unwrap();
        assert_eq!(*guard, 43);
        drop(guard);
        assert_eq!(recorded_test_events(RecordedKind::MutexContention), before + 1);
    }

    #[test]
    fn gamma_round2_native_guard_transfer_records_release_before_handing_off_ownership() {
        crate::gamma_round2_telemetry::enable_recording();
        let mutex = Mutex::new(41);
        let guard = mutex.lock();
        let before = recorded_test_events(RecordedKind::MutexRelease);
        let (raw, panicking) = guard.into_native();
        let after = recorded_test_events(RecordedKind::MutexRelease);
        mutex.raw.inner.unlock(raw);
        assert!(!panicking);
        assert_eq!(after, before + 1);
        assert_eq!(*mutex.lock(), 41);
    }
}
