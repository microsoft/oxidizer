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
    #[inline]
    pub(in crate::sync) fn new(mutex: &'a Mutex, guard: Guard<'a>) -> Self {
        Self { mutex, guard: Some(guard) }
    }

    pub(in crate::sync) fn into_guard(mut self) -> Guard<'a> {
        self.guard.take().expect("new retains raw ownership until transferred or released")
    }
}

impl Drop for Unlock<'_> {
    #[inline]
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
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

#[cfg(all(test, any(target_os = "linux", target_os = "windows")))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::AtomicU32;

    std::thread_local! {
        static ADDRESS_WAITS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn on_address_wait() {
        ADDRESS_WAITS.with(|waits| waits.set(waits.get() + 1));
    }

    #[test]
    fn shared_address_wait_dispatches_to_the_native_backend() {
        let before = ADDRESS_WAITS.get();
        // A mismatched value exercises the real syscall without parking this thread.
        super::wait_on_address(&AtomicU32::new(1), 0);
        assert_eq!(ADDRESS_WAITS.get() - before, 1);
    }
}
