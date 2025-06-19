// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmark to assess the performance of the clock. The scenario:
//! * Register 5 delays, spread across 5 seconds
//! * Advance timers 2 times to make all timers fire
//!
//! Results:
//!
//! Multi-Threaded Primitives (`Mutex`, `Arc`, `RwLock`, etc.)
//! ```time:   [1.2811 µs 1.2947 µs 1.3083 µs]```
//!
//! Single-Threaded Primitives (`RefCell`, `Cell`, etc.)
//! ```time:   [1.1773 µs 1.1902 µs 1.2043 µs]```
//!
//! The MT primitives are around 7% slower for this use-case.

use std::pin::pin;
use std::task::Context;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use oxidizer_time::runtime::{ClockDriver, InactiveClock};
use oxidizer_time::{Clock, Delay};

fn criterion_benchmark(c: &mut Criterion) {
    clock(c);
}

fn clock(c: &mut Criterion) {
    let mut group = c.benchmark_group("clock_operations");

    let (clock, driver) = InactiveClock::default().activate();

    group.bench_function("clock_operations", |b| {
        b.iter(|| {
            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            clock_operations(&clock, &driver, &mut cx);
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = criterion_benchmark
}

criterion_main!(benches);

#[expect(clippy::arithmetic_side_effects, reason = "reduces clarity")]
fn clock_operations(clock: &Clock, driver: &ClockDriver, cx: &mut Context<'_>) {
    let now = Instant::now();

    let mut delay_1 = pin!(Delay::with_clock(clock, Duration::from_secs(1)));
    _ = delay_1.as_mut().poll(cx);

    let mut delay_2 = pin!(Delay::with_clock(clock, Duration::from_secs(2)));
    _ = delay_2.as_mut().poll(cx);

    let mut delay_3 = pin!(Delay::with_clock(clock, Duration::from_secs(3)));
    _ = delay_3.as_mut().poll(cx);

    let mut delay_4 = pin!(Delay::with_clock(clock, Duration::from_secs(4)));
    _ = delay_4.as_mut().poll(cx);

    let mut delay_5 = pin!(Delay::with_clock(clock, Duration::from_secs(5)));
    _ = delay_5.as_mut().poll(cx);

    _ = driver.advance_timers(now + Duration::from_secs(2));

    _ = delay_1.as_mut().poll(cx);
    _ = delay_2.as_mut().poll(cx);

    _ = driver.advance_timers(now + Duration::from_secs(4));
    _ = delay_3.as_mut().poll(cx);
    _ = delay_4.as_mut().poll(cx);
    _ = delay_5.as_mut().poll(cx);
}