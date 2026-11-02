// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example shows how `ClockControl` controls time in a test.

use std::time::Duration;

use futures::executor::block_on;
use tick::ClockControl;

fn main() -> Result<(), ohno::AppError> {
    let control = ClockControl::new().auto_advance_timers(true);
    let clock = control.to_clock();

    // Read the current time twice.
    let now = clock.system_time();
    let later = clock.system_time();

    // Time does not move until the control advances it.
    assert_eq!(now, later);

    // Advance the clock by 1 second.
    control.advance(Duration::from_secs(1));

    // Make sure that time advanced by 1 second.
    assert_eq!(clock.system_time().duration_since(later)?, Duration::from_secs(1));

    // Create a stopwatch.
    let stopwatch = clock.stopwatch();

    // The stopwatch also stays frozen.
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(0));

    // Advance the clock by 2 seconds.
    control.advance(Duration::from_secs(2));
    assert_eq!(stopwatch.elapsed(), Duration::from_secs(2));

    // Auto-advance completes this 1000-second simulated delay without a real wait.
    block_on(clock.delay(Duration::from_secs(1000)));

    Ok(())
}
