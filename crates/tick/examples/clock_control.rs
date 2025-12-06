// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "example code")]

//! This example demonstrates how to use `ClockControl` to control the flow of time in tests.

use std::time::Duration;

use futures::executor::block_on;
use tick::{ClockControl, Delay, Stopwatch};

fn main() {
    let control = ClockControl::new().auto_advance_timers(true);
    let clock = control.to_clock();

    // Retrieve the current time.
    let now = clock.timestamp();

    // Retrieve the time again later.
    let later = clock.timestamp();

    // Notice that the time is the same.
    assert_eq!(now, later);

    // Advance the clock by 1 second.
    control.advance(Duration::from_secs(1));

    // Verify that time has advanced by 1 second.
    assert_eq!(clock.timestamp().checked_duration_since(later).unwrap(), Duration::from_secs(1));

    // Create a stopwatch.
    let stopwatch = Stopwatch::new(&clock);

    // Notice that time does not move on its own.
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(0));

    // Advance the clock by 2 seconds.
    control.advance(Duration::from_secs(2));
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(2));

    // Delay for 1000 seconds. The clock automatically advances to complete this timer
    // because `auto_advance_timers` is set to true.
    let delay = Delay::new(&clock, Duration::from_secs(1000));

    // The delay finishes immediately.
    block_on(delay);
}
