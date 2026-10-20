// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicU8, AtomicU32};
use std::time::Duration;

use windows_sys::Win32::Foundation::ERROR_TIMEOUT;
use windows_sys::Win32::System::Threading::{INFINITE, WaitOnAddress, WakeByAddressAll, WakeByAddressSingle};

pub(super) type AtomicLockWord = AtomicU8;
pub(super) type LockWord = u8;

#[cfg_attr(test, mutants::skip)] // OS waits may wake spuriously; deleting one changes parking cost, not state-loop behavior.
pub(super) fn wait_lock(state: &AtomicLockWord, expected: LockWord) {
    // SAFETY: both byte values are alive for the wait; Windows compares and
    // parks atomically, so a release before registration cannot be lost.
    let result = unsafe {
        WaitOnAddress(
            state.as_ptr().cast(),
            std::ptr::from_ref(&expected).cast(),
            size_of::<LockWord>(),
            INFINITE,
        )
    };
    verify_mutex_wait(result, std::io::Error::last_os_error);
}

pub(super) fn wake_lock(state: &AtomicLockWord) {
    // SAFETY: the byte remains alive for this wake operation.
    unsafe { WakeByAddressSingle(state.as_ptr().cast()) };
}

pub(super) fn wait_condition(state: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    #[cfg(test)]
    super::tests::on_address_wait();
    let milliseconds = milliseconds(timeout);
    // SAFETY: both aligned four-byte values remain alive during the wait.
    let result = unsafe {
        WaitOnAddress(
            state.as_ptr().cast(),
            std::ptr::from_ref(&expected).cast(),
            size_of::<u32>(),
            milliseconds,
        )
    };
    condition_wait_result(result, std::io::Error::last_os_error)
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
