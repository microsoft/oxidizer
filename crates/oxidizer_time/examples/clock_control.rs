// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This example demonstrates how to use `ClockControl` to control the flow of time in tests.

use std::error::Error;
use std::time::Duration;

use futures::executor::block_on;
use oxidizer_time::{Clock, ClockControl, Delay, Stopwatch};

fn main() -> Result<(), Box<dyn Error>> {
    let control = ClockControl::new().auto_advance_timers(true);
    let clock = Clock::with_control(&control);

    // Retrieve the current time.
    let now = clock.now();

    // Retrieve the time later.
    let later = clock.now();

    // Notice the time is the same.
    assert_eq!(now, later);

    // Advance the clock by 1 second.
    control.advance(Duration::from_secs(1));

    // Time advanced by 1 second
    assert_eq!(
        clock.now().checked_duration_since(later)?,
        Duration::from_secs(1)
    );

    // Create a stopwatch.
    let stopwatch = Stopwatch::with_clock(&clock);

    // Notice that time does not move.
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(0));

    // Advance the clock by 2 seconds.
    control.advance(Duration::from_secs(2));
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(2));

    // Delay for 1000 second, clock automatically advances to complete this timer
    // because auto_advance_timers is set to true.
    let delay = Delay::with_clock(&clock, Duration::from_secs(1000));

    // Delay finishes immediately.
    block_on(delay);

    Ok(())
}