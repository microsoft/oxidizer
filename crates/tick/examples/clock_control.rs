// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "example code")]

//! This example demonstrates how to use `ClockControl` to control the passage of time.

use std::time::Duration;

use futures::executor::block_on;
use tick::ClockControl;

fn main() {
    let control = ClockControl::new().auto_advance_timers(true);
    let clock = control.to_clock();

    // Retrieve the current time.
    let now = clock.system_time();

    // Retrieve the time again later.
    let later = clock.system_time();

    // Notice that the time is the same.
    assert_eq!(now, later);

    // Advance the clock by 1 second.
    control.advance(Duration::from_secs(1));

    // Verify that time has advanced by 1 second.
    assert_eq!(clock.system_time().duration_since(later).unwrap(), Duration::from_secs(1));

    // Create a stopwatch.
    let stopwatch = clock.stopwatch();

    // Notice that time does not move on its own.
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(0));

    // Advance the clock by 2 seconds.
    control.advance(Duration::from_secs(2));
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(2));

    // Delay for 1000 seconds. The clock automatically advances to complete this timer
    // because `auto_advance_timers` is set to true.
    // The delay finishes immediately.
    block_on(clock.delay(Duration::from_secs(1000)));
}
