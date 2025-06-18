// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This example demonstrates the basic usage of the Oxidizer time APIs with Tokio runtime.

use std::time::Duration;

use oxidizer_time::fmt::{Iso8601Timestamp, Rfc2822Timestamp};
use oxidizer_time::{Clock, Delay, Stopwatch};
use tokio::spawn;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a clock using Tokio runtime.
    let clock = Clock::tokio();

    // Use stopwatch to measure elapsed time.
    let stopwatch = Stopwatch::with_clock(&clock);

    // Delay for 2 seconds.
    Delay::with_clock(&clock, Duration::from_secs(2)).await;
    println!("Elapsed time: {}ms", stopwatch.elapsed().as_millis());

    // Delay for 2 seconds in a background task.
    let clock_clone = clock.clone();
    spawn(async move {
        let stopwatch = Stopwatch::with_clock(&clock_clone);
        Delay::with_clock(&clock_clone, Duration::from_secs(2)).await;
        println!(
            "Elapsed time (background): {}ms",
            stopwatch.elapsed().as_millis()
        );
    })
    .await?;

    // Retrieve the current time.
    let time = clock.now();

    // Use the time in various formats.
    let iso: Iso8601Timestamp = time.into();
    let rfc: Rfc2822Timestamp = time.into();

    // Print current time in various formats.
    println!("Current time (ISO 8601): {iso}");
    println!("Current time (RFC 2822): {rfc}");

    // Calculate the duration between two times.
    Delay::with_clock(&clock, Duration::from_secs(1)).await;

    let new_time = clock.now();

    // Checked duration can return an error if the first time is lesser than the second time.
    let diff = new_time.checked_duration_since(time)?;

    // Print the difference in milliseconds.
    println!("Time difference: {}ms", diff.as_millis());

    Ok(())
}