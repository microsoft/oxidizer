// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public Tokio-driver timing behavior.

#![cfg(feature = "tokio")]

use std::time::Duration;

use thread_aware::Relocator;
use tick::{Clock, Delay, PeriodicTimer};

#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test]
async fn tokio_clock_advances_timers() {
    let clock = Clock::new_tokio();

    clock.delay(Duration::from_millis(15)).await;
}

#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test]
async fn tokio_clock_advances_timers_after_relocation() {
    let mut clock = Clock::new_tokio();
    _ = Relocator::between_numa_nodes().relocate(&mut clock);

    clock.delay(Duration::from_millis(15)).await;
}

#[cfg(feature = "fast-instant")]
#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test]
async fn fast_instant_clock_keeps_driver_timer_scheduling_precise() {
    let clock = Clock::new_tokio().with_fast_instant(true);

    tokio::time::timeout(Duration::from_secs(1), Delay::new(&clock, Duration::from_millis(5)))
        .await
        .unwrap();

    let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));
    let tick = tokio::time::timeout(Duration::from_secs(1), timer.next()).await.unwrap();
    assert!(tick.observed_at() <= clock.instant());
}

#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test]
async fn live_tokio_clock_debug_output_reports_an_active_driver() {
    let clock = Clock::new_tokio();
    clock.delay(Duration::from_millis(1)).await;

    assert_eq!(
        format!("{clock:#?}"),
        "Clock {\n    kind: \"system\",\n    timers: 0,\n    alive: true,\n    ..\n}"
    );
}
