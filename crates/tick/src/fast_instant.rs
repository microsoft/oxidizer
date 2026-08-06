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
fn platform_time() -> Duration {
    let mut timestamp = std::mem::MaybeUninit::<libc::timespec>::uninit();

    // SAFETY: clock_gettime initializes the provided timespec pointer.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_COARSE, timestamp.as_mut_ptr()) };
    assert_eq!(
        result,
        0,
        "CLOCK_MONOTONIC_COARSE must be available: {}",
        std::io::Error::last_os_error()
    );

    // SAFETY: A successful clock_gettime call initialized the timespec above.
    let timestamp = unsafe { timestamp.assume_init() };
    #[expect(clippy::cast_sign_loss, reason = "monotonic clock seconds and nanoseconds are always nonnegative")]
    Duration::new(timestamp.tv_sec as u64, timestamp.tv_nsec as u32)
}

#[cfg(all(windows, not(miri)))]
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
