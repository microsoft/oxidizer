// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares standard and lower-resolution instant retrieval.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use tick::SimpleClock;

#[cfg(any(target_os = "linux", windows))]
struct UncachedSource {
    instant_epoch: Instant,
    platform_epoch: Duration,
}

#[cfg(any(target_os = "linux", windows))]
impl UncachedSource {
    fn new() -> Self {
        Self {
            instant_epoch: Instant::now(),
            platform_epoch: platform_time(),
        }
    }

    fn now(&self) -> Instant {
        self.instant_epoch
            .checked_add(platform_time().saturating_sub(self.platform_epoch))
            .expect("a monotonic platform timestamp cannot exceed the range of Instant")
    }
}

#[cfg(target_os = "linux")]
fn platform_time() -> Duration {
    let mut timestamp = std::mem::MaybeUninit::<libc::timespec>::uninit();

    // SAFETY: clock_gettime initializes the provided timespec pointer.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_COARSE, timestamp.as_mut_ptr()) };
    assert_eq!(result, 0);

    // SAFETY: A successful clock_gettime call initialized the timespec above.
    let timestamp = unsafe { timestamp.assume_init() };
    #[expect(clippy::cast_sign_loss, reason = "monotonic clock seconds and nanoseconds are always nonnegative")]
    Duration::new(timestamp.tv_sec as u64, timestamp.tv_nsec as u32)
}

#[cfg(windows)]
fn platform_time() -> Duration {
    // SAFETY: GetTickCount64 has no safety requirements.
    Duration::from_millis(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() })
}

fn criterion_benchmark(c: &mut Criterion) {
    retrieval(c);
}

fn retrieval(c: &mut Criterion) {
    let clock = SimpleClock::new_system();
    #[cfg(any(target_os = "linux", windows))]
    let uncached = UncachedSource::new();
    let mut group = c.benchmark_group("tick_instant/retrieval");

    group.bench_function("instant", |b| {
        b.iter(|| black_box(clock.instant()));
    });
    #[cfg(any(target_os = "linux", windows))]
    group.bench_function("instant_fast_uncached", |b| {
        b.iter(|| black_box(uncached.now()));
    });
    group.bench_function("instant_fast", |b| {
        b.iter(|| black_box(clock.instant_fast()));
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
