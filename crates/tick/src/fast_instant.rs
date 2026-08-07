// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
thread_local! {
    static SOURCE: std::cell::RefCell<Source> = std::cell::RefCell::new(Source::new());
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
#[derive(Debug)]
struct Source {
    instant_epoch: Instant,
    platform_epoch: Duration,
    cache_key: Duration,
    cached: Instant,
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
impl Source {
    fn new() -> Self {
        let instant_epoch = Instant::now();

        Self {
            instant_epoch,
            platform_epoch: platform_time(),
            cache_key: Duration::ZERO,
            cached: instant_epoch,
        }
    }

    fn now(&mut self) -> Instant {
        let platform_time = platform_time();

        if self.cache_key == platform_time {
            return self.cached;
        }

        let elapsed = platform_time.saturating_sub(self.platform_epoch);
        let now = self
            .instant_epoch
            .checked_add(elapsed)
            .expect("a monotonic platform timestamp cannot exceed the range of Instant");

        self.cache_key = platform_time;
        self.cached = now;

        now
    }
}

#[cfg(all(target_os = "linux", not(miri)))]
#[cfg_attr(test, mutants::skip)] // The feature is disabled by the default mutation-test profile.
fn platform_time() -> Duration {
    let mut timestamp = std::mem::MaybeUninit::<libc::timespec>::uninit();

    // SAFETY: clock_gettime initializes the provided timespec pointer.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_COARSE, timestamp.as_mut_ptr()) };
    assert_eq!(result, 0, "CLOCK_MONOTONIC_COARSE must be available");

    // SAFETY: A successful clock_gettime call initialized the timespec above.
    let timestamp = unsafe { timestamp.assume_init() };
    let seconds = u64::try_from(timestamp.tv_sec).expect("CLOCK_MONOTONIC_COARSE seconds are guaranteed to be nonnegative");
    let nanoseconds = u32::try_from(timestamp.tv_nsec).expect("clock_gettime guarantees tv_nsec is between 0 and 999,999,999");

    Duration::new(seconds, nanoseconds)
}

#[cfg(all(windows, not(miri)))]
#[cfg_attr(test, mutants::skip)] // The feature is disabled by the default mutation-test profile.
fn platform_time() -> Duration {
    // SAFETY: GetTickCount64 has no safety requirements.
    Duration::from_millis(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() })
}

#[must_use]
pub(crate) fn now() -> Instant {
    #[cfg(all(any(target_os = "linux", windows), not(miri)))]
    {
        SOURCE.with(|source| source.borrow_mut().now())
    }

    #[cfg(any(miri, not(any(target_os = "linux", windows))))]
    {
        Instant::now()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, any(target_os = "linux", windows), not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn repeated_calls_use_cached_instant() {
        let mut previous = now();

        for _ in 0..1_000 {
            let current = now();
            if current == previous {
                return;
            }
            previous = current;
        }

        panic!("the coarse platform clock must return the same timestamp for consecutive calls");
    }

    #[test]
    fn cache_refreshes_when_platform_time_advances() {
        let first = now();

        std::thread::sleep(Duration::from_millis(50));

        assert!(now() > first);
    }

    #[test]
    fn platform_time_is_nonzero() {
        assert_ne!(platform_time(), Duration::ZERO);
    }
}
