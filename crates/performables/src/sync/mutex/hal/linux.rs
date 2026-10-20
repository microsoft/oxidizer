// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::AtomicU32;
use std::time::Duration;

pub(super) type AtomicLockWord = AtomicU32;
pub(super) type LockWord = u32;

// FUTEX_WAIT is opcode zero. These wait queues never cross process boundaries.
const WAIT_PRIVATE: libc::c_int = libc::FUTEX_PRIVATE_FLAG;
const WAKE_PRIVATE: libc::c_int = libc::FUTEX_PRIVATE_FLAG + libc::FUTEX_WAKE;

#[cfg_attr(test, mutants::skip)] // OS waits may wake spuriously; deleting one changes parking cost, not state-loop behavior.
pub(super) fn wait_lock(state: &AtomicLockWord, expected: LockWord) {
    wait_condition(state, expected, None);
}

pub(super) fn wake_lock(state: &AtomicLockWord) {
    wake(state, 1);
}

pub(super) fn wait_condition(state: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    #[cfg(test)]
    super::tests::on_address_wait();
    let timeout = timeout.map(|duration| libc::timespec {
        tv_sec: duration.as_secs().try_into().unwrap_or(libc::time_t::MAX),
        // Duration's subsecond value fits a signed 32-bit integer.
        tv_nsec: libc::c_long::from(duration.subsec_nanos().cast_signed()),
    });
    // SAFETY: AtomicU32 supplies an aligned live word, and the optional relative
    // timeout remains alive. FUTEX_WAIT compares and parks atomically.
    let result = unsafe {
        libc::syscall(
            libc::SYS_futex,
            state.as_ptr(),
            WAIT_PRIVATE,
            expected,
            timeout.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
        )
    };
    wait_result(result, std::io::Error::last_os_error)
}

fn wait_result(result: libc::c_long, last_error: impl FnOnce() -> std::io::Error) -> bool {
    if result == 0 {
        return false;
    }
    let error = last_error();
    match error.raw_os_error() {
        Some(libc::EAGAIN | libc::EINTR) => false,
        Some(libc::ETIMEDOUT) => true,
        _ => super::failed("condition address wait", &error),
    }
}

pub(super) fn wake_condition_one(state: &AtomicU32) {
    wake(state, 1);
}

pub(super) fn wake_condition_all(state: &AtomicU32) {
    wake(state, libc::c_int::MAX);
}

fn wake(state: &AtomicU32, count: libc::c_int) {
    // SAFETY: the aligned word remains live; wake only inspects its wait queue.
    let result = unsafe { libc::syscall(libc::SYS_futex, state.as_ptr(), WAKE_PRIVATE, count) };
    wake_result(result, std::io::Error::last_os_error);
}

fn wake_result(result: libc::c_long, last_error: impl FnOnce() -> std::io::Error) {
    if result == -1 {
        super::failed("address wake", &last_error());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn wait_statuses_distinguish_retryable_wakes_and_timeout() {
        assert_eq!(
            (
                wait_result(0, || panic!("successful wait must not read errno")),
                wait_result(-1, || std::io::Error::from_raw_os_error(libc::EAGAIN)),
                wait_result(-1, || std::io::Error::from_raw_os_error(libc::EINTR)),
                wait_result(-1, || std::io::Error::from_raw_os_error(libc::ETIMEDOUT)),
            ),
            (false, false, false, true)
        );
    }

    #[test]
    #[should_panic(expected = "condition address wait failed:")]
    fn unexpected_wait_error_is_not_swallowed() {
        wait_result(-1, || std::io::Error::other("unexpected kernel failure"));
    }

    #[test]
    fn successful_wake_does_not_read_errno() {
        wake_result(0, || panic!("successful wake must not read errno"));
    }

    #[test]
    #[should_panic(expected = "address wake failed:")]
    fn unexpected_wake_error_is_not_swallowed() {
        wake_result(-1, || std::io::Error::other("unexpected kernel failure"));
    }
}
