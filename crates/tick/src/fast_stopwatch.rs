// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lower-overhead timestamps for fast stopwatches.

use std::time::Duration;
#[cfg(any(miri, not(any(target_os = "linux", windows))))]
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Timestamp(
    #[cfg(all(any(target_os = "linux", windows), not(miri)))] u64,
    #[cfg(any(miri, not(any(target_os = "linux", windows))))] Instant,
);

impl Timestamp {
    pub(crate) fn elapsed_since(self, start: Self) -> Duration {
        #[cfg(all(any(target_os = "linux", windows), not(miri)))]
        {
            Duration::from_nanos(self.0.saturating_sub(start.0))
        }

        #[cfg(any(miri, not(any(target_os = "linux", windows))))]
        {
            self.0.saturating_duration_since(start.0)
        }
    }
}

#[cfg(all(target_os = "linux", not(miri)))]
fn platform_time() -> u64 {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;

    let mut timestamp = std::mem::MaybeUninit::<libc::timespec>::uninit();

    // SAFETY: `as_mut_ptr` is non-null, aligned, and writable for one `timespec`.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_COARSE, timestamp.as_mut_ptr()) };
    assert_eq!(result, 0, "CLOCK_MONOTONIC_COARSE predates Rust's minimum supported Linux kernel");

    // SAFETY: The checked successful return guarantees that `timestamp` was initialized.
    let timestamp = unsafe { timestamp.assume_init() };
    let seconds = u64::try_from(timestamp.tv_sec).expect("CLOCK_MONOTONIC_COARSE seconds are guaranteed to be nonnegative");
    let nanoseconds = u64::try_from(timestamp.tv_nsec).expect("clock_gettime guarantees tv_nsec is between 0 and 999,999,999");

    // Nanoseconds in u64 cover more than 584 years of uptime. Saturation keeps
    // the timestamp monotonic beyond that range.
    seconds.saturating_mul(NANOS_PER_SECOND).saturating_add(nanoseconds)
}

#[cfg(all(windows, not(miri)))]
fn platform_time() -> u64 {
    const NANOS_PER_MILLISECOND: u64 = 1_000_000;

    // SAFETY: GetTickCount64 has no safety requirements.
    let milliseconds = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    milliseconds.saturating_mul(NANOS_PER_MILLISECOND)
}

#[must_use]
pub(crate) fn now() -> Timestamp {
    #[cfg(all(any(target_os = "linux", windows), not(miri)))]
    {
        Timestamp(platform_time())
    }

    #[cfg(any(miri, not(any(target_os = "linux", windows))))]
    {
        Timestamp(Instant::now())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, any(target_os = "linux", windows), not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_saturates_for_out_of_order_samples() {
        assert_eq!(Timestamp(15).elapsed_since(Timestamp(10)), Duration::from_nanos(5));
        assert_eq!(Timestamp(10).elapsed_since(Timestamp(15)), Duration::ZERO);
    }

    #[test]
    fn platform_time_advances() {
        let start = platform_time();
        // Allow the coarse platform clock to advance by at least one tick.
        std::thread::sleep(Duration::from_millis(100));
        assert!(platform_time() > start);
    }
}
