// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::sync::mutex::hal::{wait_on_address, wake_all_at_address};

// One high bit registers potential sleepers; the remaining bits count readers.
// The all-ones owner count represents a single writer, never readers.
const WAITERS: u32 = 1 << 31;
const OWNERS: u32 = WAITERS - 1;
const WRITER: u32 = OWNERS;
const MAX_READERS: u32 = WRITER - 1;
const WRITER_WAITING: u32 = u32::MAX;

// Wrapping std::RwLock<()> retains a second owning guard and poison snapshot.
// One crate-owned word avoids measured guard/Result copies; the existing safe
// address-wait adapters supply parking without introducing more platform FFI.
#[derive(Debug)]
pub(in crate::sync::lock) struct RwLock {
    state: AtomicU32,
    poisoned: AtomicBool,
}

#[derive(Debug)]
pub(in crate::sync::lock) struct ReadGuard<'a> {
    // Only the affinity of std's guard is retained, not a native owning guard.
    marker: PhantomData<std::sync::RwLockReadGuard<'a, ()>>,
}

#[derive(Debug)]
pub(in crate::sync::lock) struct WriteGuard<'a> {
    marker: PhantomData<std::sync::RwLockWriteGuard<'a, ()>>,
}

impl RwLock {
    pub(in crate::sync::lock) const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            poisoned: AtomicBool::new(false),
        }
    }

    #[inline]
    pub(in crate::sync::lock) fn try_read(&self) -> Option<ReadGuard<'_>> {
        let mut state = self.state.load(Ordering::Relaxed);
        while state < MAX_READERS {
            match self
                .state
                .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return Some(ReadGuard { marker: PhantomData }),
                Err(current) => state = current,
            }
        }
        None
    }

    pub(in crate::sync::lock) fn read(&self) -> ReadGuard<'_> {
        if let Some(guard) = self.try_read() {
            return guard;
        }
        self.read_contended()
    }

    #[cold]
    fn read_contended(&self) -> ReadGuard<'_> {
        self.read_contended_with(|| self.try_read(), wait_on_address)
    }

    #[cold]
    fn read_contended_with<'a>(
        &'a self,
        mut try_read: impl FnMut() -> Option<ReadGuard<'a>>,
        mut wait: impl FnMut(&AtomicU32, u32),
    ) -> ReadGuard<'a> {
        loop {
            if let Some(guard) = try_read() {
                return guard;
            }
            let state = self.state.load(Ordering::Relaxed);
            assert_ne!(state & OWNERS, MAX_READERS, "too many simultaneous RwLock readers");
            if state < MAX_READERS {
                continue;
            }
            if self
                .state
                .compare_exchange(state, state | WAITERS, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                // Registration precedes the atomic expected-value park.
                wait(&self.state, state | WAITERS);
            }
        }
    }

    #[inline]
    pub(in crate::sync::lock) fn try_write(&self) -> Option<WriteGuard<'_>> {
        self.state
            .compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| WriteGuard { marker: PhantomData })
    }

    pub(in crate::sync::lock) fn write(&self) -> WriteGuard<'_> {
        if let Some(guard) = self.try_write() {
            return guard;
        }
        self.write_contended()
    }

    #[cold]
    fn write_contended(&self) -> WriteGuard<'_> {
        self.write_contended_with(
            |state| {
                self.state
                    .compare_exchange_weak(state, WRITER_WAITING, Ordering::Acquire, Ordering::Relaxed)
            },
            wait_on_address,
        )
    }

    #[cold]
    fn write_contended_with(
        &self,
        mut acquire: impl FnMut(u32) -> Result<u32, u32>,
        mut wait: impl FnMut(&AtomicU32, u32),
    ) -> WriteGuard<'_> {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & OWNERS == 0 {
                // Retain registration for other sleepers until this writer releases.
                match acquire(state) {
                    Ok(_) => return WriteGuard { marker: PhantomData },
                    Err(current) => state = current,
                }
            } else {
                match self
                    .state
                    .compare_exchange(state, state | WAITERS, Ordering::Relaxed, Ordering::Relaxed)
                {
                    Ok(_) => {
                        wait(&self.state, state | WAITERS);
                        state = self.state.load(Ordering::Relaxed);
                    }
                    Err(current) => state = current,
                }
            }
        }
    }

    #[inline]
    pub(in crate::sync::lock) fn read_unlock(&self, _guard: &mut ReadGuard<'_>) {
        let previous = self.state.fetch_sub(1, Ordering::Release);
        debug_assert!(previous & OWNERS > 0 && previous & OWNERS != WRITER);
        if previous == WAITERS + 1 {
            self.read_unlock_contended();
        }
    }

    #[cold]
    fn read_unlock_contended(&self) {
        if self
            .state
            .compare_exchange(WAITERS, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            wake_all_at_address(&self.state);
        }
        // If a writer acquired WAITERS first, it retains the flag and wakes
        // everyone on release. Clearing cannot erase a new reader registration.
    }

    #[inline]
    pub(in crate::sync::lock) fn write_unlock(&self, guard: &mut WriteGuard<'_>) {
        self.write_unlock_with(guard, wake_all_at_address);
    }

    #[inline]
    fn write_unlock_with(&self, _guard: &mut WriteGuard<'_>, wake: impl FnOnce(&AtomicU32)) {
        let previous = self.state.swap(0, Ordering::Release);
        debug_assert_eq!(previous & OWNERS, WRITER);
        if previous & WAITERS != 0 {
            // Readers and writers share the address queue. Waking all avoids
            // selecting a reader that would leave the only eligible writer asleep.
            wake(&self.state);
        }
    }

    #[inline]
    pub(in crate::sync::lock) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    pub(in crate::sync::lock) fn clear_poison(&self) -> bool {
        self.poisoned.swap(false, Ordering::Relaxed)
    }

    pub(in crate::sync::lock) fn prepare_poison(&self) -> bool {
        // The subsequent ownership release publishes this state to acquirers.
        !self.poisoned.swap(true, Ordering::Relaxed)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn shared_and_exclusive_ownership_can_be_reused() {
        let lock = RwLock::new();
        let mut first = lock.try_read().unwrap();
        let mut second = lock.read();
        let writer_blocked = lock.try_write().is_none();
        lock.read_unlock(&mut first);
        lock.read_unlock(&mut second);
        let mut writer = lock.write();
        let reader_blocked = lock.try_read().is_none();
        let other_writer_blocked = lock.try_write().is_none();
        lock.write_unlock(&mut writer);
        let mut reader = lock.try_read().unwrap();
        lock.read_unlock(&mut reader);
        assert_eq!(
            (
                writer_blocked,
                reader_blocked,
                other_writer_blocked,
                lock.state.load(Ordering::Relaxed)
            ),
            (true, true, true, 0)
        );
    }

    #[test]
    fn registered_writer_owns_the_next_wake_when_it_wins_the_handoff() {
        let lock = RwLock::new();
        lock.state.store(WAITERS, Ordering::Relaxed);
        let mut writer = lock.write();
        lock.read_unlock_contended();
        assert_eq!(lock.state.load(Ordering::Relaxed), WRITER | WAITERS);
        lock.write_unlock(&mut writer);
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn last_reader_clears_empty_waiter_registration() {
        let lock = RwLock::new();
        let mut reader = lock.read();
        lock.state.fetch_or(WAITERS, Ordering::Relaxed);
        lock.read_unlock(&mut reader);
        let mut writer = lock.try_write().unwrap();
        lock.write_unlock(&mut writer);
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reader_parks_at_the_registered_state_after_a_spurious_wake() {
        let lock = RwLock::new();
        let mut writer = Some(lock.write());
        let mut waits = Vec::new();
        let mut reader = lock.read_contended_with(
            || lock.try_read(),
            |state, expected| {
                let observed = state.load(Ordering::Relaxed);
                waits.push((observed & WAITERS != 0, expected == observed));
                if waits.len() == 2 {
                    lock.write_unlock(&mut writer.take().unwrap());
                }
            },
        );
        lock.read_unlock(&mut reader);
        assert_eq!((waits, lock.state.load(Ordering::Relaxed)), (vec![(true, true); 2], 0));
    }

    #[test]
    fn reader_does_not_park_if_the_writer_releases_between_probes() {
        let lock = RwLock::new();
        let mut writer = Some(lock.write());
        let mut reader = lock.read_contended_with(
            || {
                let acquired = lock.try_read();
                if let Some(mut writer) = writer.take() {
                    lock.write_unlock(&mut writer);
                }
                acquired
            },
            |_, _| panic!("no owner remains to wake a parked reader"),
        );
        lock.read_unlock(&mut reader);
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn writer_preserves_registration_across_spurious_wakes() {
        let lock = RwLock::new();
        let mut owner = Some(lock.write());
        let mut waits = Vec::new();
        let mut writer = lock.write_contended_with(
            |state| {
                lock.state
                    .compare_exchange_weak(state, WRITER_WAITING, Ordering::Acquire, Ordering::Relaxed)
            },
            |state, expected| {
                let observed = state.load(Ordering::Relaxed);
                waits.push((observed & WAITERS != 0, expected == observed));
                if waits.len() == 2 {
                    lock.write_unlock(&mut owner.take().unwrap());
                }
            },
        );
        lock.write_unlock(&mut writer);
        assert_eq!((waits, lock.state.load(Ordering::Relaxed)), (vec![(true, true); 2], 0));
    }

    #[test]
    fn writer_waits_for_the_owner_that_wins_an_empty_handoff() {
        let lock = RwLock::new();
        let mut attempts = 0;
        let mut waits = Vec::new();
        let mut writer = lock.write_contended_with(
            |state| {
                attempts += 1;
                assert!(attempts <= 2);
                if attempts == 1 {
                    // A rival writer wins between the initial load and this CAS.
                    lock.state.store(WRITER_WAITING, Ordering::Relaxed);
                }
                lock.state
                    .compare_exchange(state, WRITER_WAITING, Ordering::Acquire, Ordering::Relaxed)
            },
            |state, expected| {
                waits.push((expected, state.load(Ordering::Relaxed)));
                state.store(0, Ordering::Release);
            },
        );
        lock.write_unlock(&mut writer);
        assert_eq!(
            (attempts, waits, lock.state.load(Ordering::Relaxed)),
            (2, vec![(WRITER_WAITING, WRITER_WAITING)], 0)
        );
    }

    #[test]
    fn writer_only_wakes_registered_waiters_after_releasing_ownership() {
        let lock = RwLock::new();
        let mut writer = lock.write();
        let mut wakes = Vec::new();
        lock.write_unlock_with(&mut writer, |state| wakes.push(state.load(Ordering::Relaxed)));
        assert!(wakes.is_empty());
        let mut writer = lock.write();
        lock.state.fetch_or(WAITERS, Ordering::Relaxed);
        lock.write_unlock_with(&mut writer, |state| wakes.push(state.load(Ordering::Relaxed)));
        assert_eq!((wakes, lock.state.load(Ordering::Relaxed)), (vec![0], 0));
    }

    #[test]
    #[should_panic(expected = "too many simultaneous RwLock readers")]
    fn reader_overflow_is_rejected_without_becoming_a_writer() {
        let lock = RwLock::new();
        lock.state.store(MAX_READERS, Ordering::Relaxed);
        lock.read();
    }
}
