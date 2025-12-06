// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates the basic usage of Clock with the Tokio runtime.

use std::time::Duration;

use tick::fmt::{Iso8601Timestamp, Rfc2822Timestamp};
use tick::{Clock, Delay, Stopwatch};
use tokio::spawn;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a clock using the Tokio runtime.
    let clock = Clock::new_tokio();

    // Use a stopwatch to measure elapsed time.
    let stopwatch = Stopwatch::new(&clock);

    // Delay for 2 seconds.
    Delay::new(&clock, Duration::from_secs(2)).await;
    println!("Elapsed time: {}ms", stopwatch.elapsed().as_millis());

    // Delay for 2 seconds in a background task.
    let clock_clone = clock.clone();
    spawn(async move {
        let stopwatch = Stopwatch::new(&clock_clone);
        Delay::new(&clock_clone, Duration::from_secs(2)).await;
        println!("Elapsed time (background): {}ms", stopwatch.elapsed().as_millis());
    })
    .await?;

    // Retrieve the current time.
    let time = clock.timestamp();

    // Convert the time to various formats.
    let iso: Iso8601Timestamp = time.into();
    let rfc: Rfc2822Timestamp = time.into();

    // Print the current time in various formats.
    println!("Current time (ISO 8601): {iso}");
    println!("Current time (RFC 2822): {rfc}");

    // Calculate the duration between two timestamps.
    Delay::new(&clock, Duration::from_secs(1)).await;

    let new_time = clock.timestamp();

    // Checked duration can return an error if the first timestamp is earlier than the second timestamp.
    let diff = new_time.checked_duration_since(time)?;

    // Print the difference in milliseconds.
    println!("Time difference: {}ms", diff.as_millis());

    Ok(())
}
