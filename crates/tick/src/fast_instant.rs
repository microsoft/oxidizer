// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
use std::time::Duration;
use std::time::Instant;

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
thread_local! {
    static SOURCE: std::cell::RefCell<Source> = std::cell::RefCell::new(Source::new(*calibration()));
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
static CALIBRATION: std::sync::OnceLock<Calibration> = std::sync::OnceLock::new();

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
#[derive(Clone, Copy, Debug)]
struct Calibration {
    instant_epoch: Instant,
    platform_epoch: Duration,
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
fn calibration() -> &'static Calibration {
    CALIBRATION.get_or_init(|| Calibration {
        instant_epoch: Instant::now(),
        platform_epoch: platform_time(),
    })
}

/// Maps coarse platform timestamps into the process-wide [`Instant`] comparison domain.
///
/// Every instance uses the same calibration pair, so values remain comparable when clocks or
/// stopwatches move between threads. Each thread keeps its own cache because repeated platform
/// timestamps are common and avoiding reconstruction is important on the hot path.
///
/// The cache key and cached instant always describe the same sample. Equal or unexpectedly older
/// samples reuse the cached value, preserving non-decreasing results. Newer samples are translated
/// by adding their elapsed platform duration to the calibrated `Instant`.
#[cfg(all(any(target_os = "linux", windows), not(miri)))]
#[derive(Debug)]
struct Source {
    calibration: Calibration,
    cache_key: Duration,
    cached: Instant,
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
impl Source {
    fn new(calibration: Calibration) -> Self {
        Self {
            calibration,
            cache_key: calibration.platform_epoch,
            cached: calibration.instant_epoch,
        }
    }

    fn now(&mut self) -> Instant {
        self.now_at(platform_time())
    }

    fn now_at(&mut self, platform_time: Duration) -> Instant {
        if platform_time <= self.cache_key {
            return self.cached;
        }

        let elapsed = platform_time.saturating_sub(self.calibration.platform_epoch);
        let now = self
            .calibration
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

    // SAFETY: `as_mut_ptr` is non-null, aligned, and writable for one `timespec`.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_COARSE, timestamp.as_mut_ptr()) };
    assert_eq!(result, 0, "CLOCK_MONOTONIC_COARSE must be available");

    // SAFETY: The checked successful return guarantees that `timestamp` was initialized.
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
    fn cache_and_conversion_are_deterministic() {
        let instant_epoch = Instant::now();
        let platform_epoch = Duration::from_secs(10);
        let mut source = Source::new(Calibration {
            instant_epoch,
            platform_epoch,
        });

        assert_eq!(source.now_at(platform_epoch), instant_epoch);

        let advanced_platform = platform_epoch + Duration::from_millis(5);
        let advanced = source.now_at(advanced_platform);
        assert_eq!(advanced, instant_epoch + Duration::from_millis(5));
        assert_eq!(source.now_at(advanced_platform), advanced);

        assert_eq!(source.now_at(platform_epoch), advanced);
    }

    #[test]
    fn shared_calibration_keeps_thread_results_comparable() {
        let calibration = Calibration {
            instant_epoch: Instant::now(),
            platform_epoch: Duration::from_secs(10),
        };

        let first = std::thread::spawn(move || Source::new(calibration).now_at(calibration.platform_epoch + Duration::from_millis(1)))
            .join()
            .expect("test thread must complete");
        let second = std::thread::spawn(move || Source::new(calibration).now_at(calibration.platform_epoch + Duration::from_millis(2)))
            .join()
            .expect("test thread must complete");

        assert!(second > first);
    }

    #[test]
    fn platform_time_can_be_read() {
        _ = platform_time();
    }
}
