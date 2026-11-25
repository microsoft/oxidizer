// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example shows the basic `tick` APIs.

use std::time::Duration;

use jiff::Timestamp;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Use a stopwatch to measure elapsed time.
    let stopwatch = clock.stopwatch();

    // Delay for 2 seconds.
    clock.delay(Duration::from_secs(2)).await;
    println!("Elapsed time: {}ms", stopwatch.elapsed().as_millis());

    // Retrieve the current time.
    let time = clock.system_time();

    // Convert the time to Jiff for parsing and formatting.
    let timestamp = Timestamp::try_from(time)?;

    println!("Current time: {timestamp}");

    // Measure the duration between two times.
    clock.delay(Duration::from_secs(1)).await;

    let new_time = clock.system_time();

    // `duration_since` returns an error if `new_time` is earlier than `time`.
    let diff = new_time.duration_since(time)?;

    // Print the duration in milliseconds.
    println!("Time difference: {}ms", diff.as_millis());

    Ok(())
}
