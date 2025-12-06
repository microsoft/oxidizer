// Copyright (c) Microsoft Corporation.

//! This example demonstrates the basic usage of tick APIs.

use std::time::Duration;

use tick::fmt::{Iso8601Timestamp, Rfc2822Timestamp};
use tick::{Clock, Delay, Stopwatch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Use a stopwatch to measure elapsed time.
    let stopwatch = Stopwatch::new(&clock);

    // Delay for 2 seconds.
    Delay::new(&clock, Duration::from_secs(2)).await;
    println!("Elapsed time: {}ms", stopwatch.elapsed().as_millis());

    // Retrieve the current time.
    let time = clock.timestamp();

    // Convert the time to various formats.
    let iso: Iso8601Timestamp = time.into();
    let rfc: Rfc2822Timestamp = time.into();

    // Print the current time in various formats.
    println!("Current time (ISO 8601): {iso}");
    println!("Current time (RFC 2822): {rfc}");

    // Calculate the duration between two times.
    Delay::new(&clock, Duration::from_secs(1)).await;

    let new_time = clock.timestamp();

    // A checked duration calculation returns an error if the first time is earlier than the second time.
    let diff = new_time.checked_duration_since(time)?;

    // Print the difference in milliseconds.
    println!("Time difference: {}ms", diff.as_millis());

    Ok(())
}
