// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod address;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(in crate::sync) use address::{Condvar, Guard, Mutex};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(any(test, not(any(target_os = "linux", target_os = "windows"))))]
mod portable;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(in crate::sync) use portable::{Condvar, Guard, Mutex};

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[cfg_attr(coverage_nightly, coverage(off))] // OS parking cost is benchmarked; native tests cover the state/wake protocols.
pub(in crate::sync) fn wait_on_address(state: &std::sync::atomic::AtomicU32, expected: u32) {
    #[cfg(target_os = "linux")]
    linux::wait_condition(state, expected, None);
    #[cfg(target_os = "windows")]
    windows::wait_condition(state, expected, None);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[cold]
pub(in crate::sync) fn wake_all_at_address(state: &std::sync::atomic::AtomicU32) {
    #[cfg(target_os = "linux")]
    linux::wake_condition_all(state);
    #[cfg(target_os = "windows")]
    windows::wake_condition_all(state);
}

/// Releases raw ownership even when a telemetry callback unwinds.
pub(in crate::sync) struct Unlock<'a> {
    mutex: &'a Mutex,
    guard: Option<Guard<'a>>,
}

impl<'a> Unlock<'a> {
    pub(in crate::sync) fn new(mutex: &'a Mutex, guard: Guard<'a>) -> Self {
        Self { mutex, guard: Some(guard) }
    }

    pub(in crate::sync) fn into_guard(mut self) -> Guard<'a> {
        self.guard.take().expect("new retains raw ownership until transferred or released")
    }
}

impl Drop for Unlock<'_> {
    // #[gamma::skip(fn_value.unit, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            self.mutex.unlock(guard);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[cold]
#[track_caller]
#[expect(clippy::panic, reason = "a failed native primitive cannot yield a valid synchronization result")]
fn failed(operation: &str, error: &std::io::Error) -> ! {
    panic!("{operation} failed: {error}");
}

#[cfg(test)]
mod gamma_survivor_tests {
    use super::{Mutex, Unlock};

    #[test]
    fn gamma_survivor_unlock_drop_releases_ownership_but_transfer_keeps_it() {
        let mutex = Mutex::new();
        let ownership = Unlock::new(&mutex, mutex.try_lock().unwrap());
        assert!(ownership.guard.is_some());
        assert!(mutex.try_lock().is_none());
        drop(ownership);
        let ownership = Unlock::new(&mutex, mutex.try_lock().unwrap());
        let transferred = ownership.into_guard();
        assert!(mutex.try_lock().is_none());
        mutex.unlock(transferred);
        let reacquired = mutex.try_lock().unwrap();
        mutex.unlock(reacquired);
    }
}
