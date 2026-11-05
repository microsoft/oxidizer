// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicU8, AtomicU32};
use std::time::Duration;

#[cfg(test)]
use injection::wait_on_address as WaitOnAddress;
use windows_sys::Win32::Foundation::ERROR_TIMEOUT;
#[cfg(not(test))]
use windows_sys::Win32::System::Threading::WaitOnAddress;
use windows_sys::Win32::System::Threading::{INFINITE, WakeByAddressAll, WakeByAddressSingle};

pub(super) type AtomicLockWord = AtomicU8;
pub(super) type LockWord = u8;

pub(super) fn wait_lock(state: &AtomicLockWord, expected: LockWord) {
    let result = native_lock_wait(state, expected);
    verify_mutex_wait(result, last_wait_error);
}

fn native_lock_wait(state: &AtomicLockWord, expected: LockWord) -> i32 {
    // SAFETY: both byte values are alive for the wait; Windows compares and
    // parks atomically, so a release before registration cannot be lost.
    unsafe {
        WaitOnAddress(
            state.as_ptr().cast(),
            std::ptr::from_ref(&expected).cast(),
            size_of::<LockWord>(),
            INFINITE,
        )
    }
}

pub(super) fn wake_lock(state: &AtomicLockWord) {
    // SAFETY: the byte remains alive for this wake operation.
    unsafe { WakeByAddressSingle(state.as_ptr().cast()) };
}

// #[gamma::skip(parameter.default_shadow, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
pub(super) fn wait_condition(state: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    let milliseconds = milliseconds(timeout);
    let result = native_condition_wait(state, expected, milliseconds);
    condition_wait_result(result, last_wait_error)
}

fn native_condition_wait(state: &AtomicU32, expected: u32, milliseconds: u32) -> i32 {
    // SAFETY: both aligned four-byte values remain alive during the wait.
    unsafe {
        WaitOnAddress(
            state.as_ptr().cast(),
            std::ptr::from_ref(&expected).cast(),
            size_of::<u32>(),
            milliseconds,
        )
    }
}

fn last_wait_error() -> std::io::Error {
    #[cfg(test)]
    if let Some(error) = injection::error() {
        return std::io::Error::from_raw_os_error(error);
    }
    std::io::Error::last_os_error()
}

#[cfg(test)]
pub(super) mod injection {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;

    use super::*;

    type LockWait = Box<dyn FnMut(&AtomicLockWord, LockWord) -> i32>;
    type ConditionWait = Box<dyn FnMut(&AtomicU32, u32, u32) -> i32>;

    thread_local! {
        static LOCK: RefCell<Option<LockWait>> = const { RefCell::new(None) };
        static CONDITION: RefCell<Option<ConditionWait>> = const { RefCell::new(None) };
        static ERROR: Cell<Option<i32>> = const { Cell::new(None) };
    }

    pub(super) unsafe fn wait_on_address(address: *const c_void, compare: *const c_void, size: usize, milliseconds: u32) -> i32 {
        let injected = if size == size_of::<LockWord>() {
            // SAFETY: the byte-wait caller supplies its live atomic at this address.
            let state = unsafe { &*address.cast::<AtomicLockWord>() };
            // SAFETY: the comparison points to the caller's live byte.
            let expected = unsafe { *compare.cast::<LockWord>() };
            assert_eq!(milliseconds, INFINITE);
            LOCK.with_borrow_mut(|callback| callback.as_mut().map(|callback| callback(state, expected)))
        } else {
            assert_eq!(size, size_of::<u32>());
            // SAFETY: the condition-wait caller supplies its live, aligned atomic.
            let state = unsafe { &*address.cast::<AtomicU32>() };
            // SAFETY: the comparison points to the caller's live, aligned word.
            let expected = unsafe { *compare.cast::<u32>() };
            CONDITION.with_borrow_mut(|callback| callback.as_mut().map(|callback| callback(state, expected, milliseconds)))
        };
        injected.unwrap_or_else(|| {
            // SAFETY: the original callers keep both correctly sized values alive for the wait.
            unsafe { windows_sys::Win32::System::Threading::WaitOnAddress(address, compare, size, milliseconds) }
        })
    }

    pub(super) fn error() -> Option<i32> {
        ERROR.get()
    }

    pub(in crate::sync::mutex::hal) fn with_waits(
        lock: impl FnMut(&AtomicLockWord, LockWord) -> i32 + 'static,
        condition: impl FnMut(&AtomicU32, u32, u32) -> i32 + 'static,
        error: Option<i32>,
        run: impl FnOnce(),
    ) {
        struct Reset(Option<LockWait>, Option<ConditionWait>, Option<i32>);
        impl Drop for Reset {
            fn drop(&mut self) {
                LOCK.with_borrow_mut(|callback| *callback = self.0.take());
                CONDITION.with_borrow_mut(|callback| *callback = self.1.take());
                ERROR.set(self.2);
            }
        }
        let _reset = Reset(
            LOCK.replace(Some(Box::new(lock))),
            CONDITION.replace(Some(Box::new(condition))),
            ERROR.replace(error),
        );
        run();
    }
}

fn milliseconds(timeout: Option<Duration>) -> u32 {
    timeout.map_or(INFINITE, |duration| {
        let rounded_up = duration.as_millis() + u128::from(duration.subsec_nanos() % 1_000_000 != 0);
        u32::try_from(rounded_up.min(u128::from(INFINITE - 1))).expect("rounded milliseconds are capped below INFINITE and fit u32")
    })
}

fn verify_mutex_wait(result: i32, last_error: impl FnOnce() -> std::io::Error) {
    if result == 0 {
        super::failed("mutex address wait", &last_error());
    }
}

fn condition_wait_result(result: i32, last_error: impl FnOnce() -> std::io::Error) -> bool {
    if result != 0 {
        return false;
    }
    let error = last_error();
    if error.raw_os_error() == Some(ERROR_TIMEOUT.cast_signed()) {
        true
    } else {
        super::failed("condition address wait", &error)
    }
}

pub(super) fn wake_condition_one(state: &AtomicU32) {
    // SAFETY: the four-byte word remains alive for the call.
    unsafe { WakeByAddressSingle(state.as_ptr().cast()) };
}

pub(super) fn wake_condition_all(state: &AtomicU32) {
    // SAFETY: the four-byte word remains alive for the call.
    unsafe { WakeByAddressAll(state.as_ptr().cast()) };
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn finite_timeouts_round_up_without_becoming_infinite() {
        assert_eq!(
            [
                milliseconds(None),
                milliseconds(Some(Duration::ZERO)),
                milliseconds(Some(Duration::from_nanos(1))),
                milliseconds(Some(Duration::from_micros(999))),
                milliseconds(Some(Duration::from_millis(1))),
                milliseconds(Some(Duration::from_nanos(1_000_001))),
                milliseconds(Some(Duration::from_millis(u64::from(INFINITE)))),
                milliseconds(Some(Duration::MAX)),
            ],
            [INFINITE, 0, 1, 1, 1, 2, INFINITE - 1, INFINITE - 1]
        );
    }

    #[test]
    fn nov4_address_wait_adapter_forwards_the_exact_state_expected_value_and_infinite_timeout() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = std::rc::Rc::clone(&calls);
        injection::with_waits(
            |_, _| panic!("the four-byte address wait must not use the byte mutex adapter"),
            move |state, expected, milliseconds| {
                assert_eq!(state.load(std::sync::atomic::Ordering::Relaxed), 19);
                assert_eq!(expected, 19);
                assert_eq!(milliseconds, INFINITE);
                observed.set(observed.get() + 1);
                1
            },
            None,
            || super::super::wait_on_address(&AtomicU32::new(19), 19),
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn nov4_byte_mutex_adapter_dispatches_once_and_does_not_swallow_native_failure() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = std::rc::Rc::clone(&calls);
        injection::with_waits(
            move |state, expected| {
                assert_eq!(state.load(std::sync::atomic::Ordering::Relaxed), 2);
                assert_eq!(expected, 2);
                observed.set(observed.get() + 1);
                0
            },
            |_, _, _| panic!("byte mutex must not use the condition adapter"),
            Some(5),
            || {
                let outcome = std::panic::catch_unwind(|| wait_lock(&AtomicLockWord::new(2), 2));
                let panic = outcome.unwrap_err();
                let message = panic.downcast_ref::<String>().unwrap();
                assert!(message.starts_with("mutex address wait failed:"));
            },
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn successful_waits_do_not_inspect_stale_errors() {
        verify_mutex_wait(1, || panic!("successful wait must not read the last error"));
        assert!(!condition_wait_result(1, || panic!("successful wait must not read the last error")));
    }

    #[test]
    fn timed_wait_reports_the_native_timeout() {
        assert!(condition_wait_result(0, || std::io::Error::from_raw_os_error(
            ERROR_TIMEOUT.cast_signed()
        )));
    }

    #[test]
    #[should_panic(expected = "mutex address wait failed:")]
    fn unexpected_mutex_wait_error_is_not_swallowed() {
        verify_mutex_wait(0, || std::io::Error::other("unexpected kernel failure"));
    }

    #[test]
    #[should_panic(expected = "condition address wait failed:")]
    fn unexpected_condition_wait_error_is_not_swallowed() {
        condition_wait_result(0, || std::io::Error::other("unexpected kernel failure"));
    }
}
