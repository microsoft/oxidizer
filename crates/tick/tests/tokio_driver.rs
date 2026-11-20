// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public Tokio-driver timing behavior.

#![cfg(feature = "tokio")]

use std::time::Duration;

use thread_aware::Relocator;
use tick::Clock;

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
