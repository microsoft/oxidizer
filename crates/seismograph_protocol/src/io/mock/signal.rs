// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Condvar, LockResult, MutexGuard, PoisonError};
use std::time::Duration;

#[derive(Debug, Default)]
pub(super) struct Signal {
    native: Condvar,
    #[cfg(test)]
    notifications: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    timed_waits: std::sync::Mutex<Vec<Duration>>,
    #[cfg(test)]
    bypass_timed_wait: std::sync::atomic::AtomicBool,
}

impl Signal {
    pub(super) fn notify_all(&self) {
        #[cfg(test)]
        self.notifications.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.native.notify_all();
    }

    pub(super) fn wait<'a, T>(&self, state: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
        self.native.wait(state)
    }

    pub(super) fn wait_timeout<'a, T>(&self, state: MutexGuard<'a, T>, timeout: Duration) -> LockResult<MutexGuard<'a, T>> {
        #[cfg(test)]
        {
            self.timed_waits.lock().unwrap().push(timeout);
            if self.bypass_timed_wait.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(state);
            }
        }
        self.native
            .wait_timeout(state, timeout)
            .map(|(state, _)| state)
            .map_err(|error| PoisonError::new(error.into_inner().0))
    }

    #[cfg(test)]
    pub(super) fn notification_count(&self) -> usize {
        self.notifications.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn bypass_timed_wait(&self) {
        self.bypass_timed_wait.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn timed_waits(&self) -> Vec<Duration> {
        self.timed_waits.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::Signal;

    #[test]
    fn timed_wait_preserves_poisoned_state() {
        let state = Mutex::new(0);
        assert!(
            catch_unwind(|| {
                let mut guard = state.lock().unwrap();
                *guard = 42;
                panic!("poison the state");
            })
            .is_err()
        );
        let guard = state.lock().unwrap_err().into_inner();
        let signal = Signal::default();
        let error = signal.wait_timeout(guard, Duration::ZERO).unwrap_err();
        assert_eq!(*error.into_inner(), 42);
        assert!(state.is_poisoned());
    }
}
