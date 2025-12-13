// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates the basic usage of tick APIs.

use std::time::Duration;

use tick::Clock;
use tick::fmt::{Iso8601, Rfc2822};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Use a stopwatch to measure elapsed time.
    let stopwatch = clock.stopwatch();

    // Delay for 2 seconds.
    clock.delay(Duration::from_secs(2)).await;
    println!("Elapsed time: {}ms", stopwatch.elapsed().as_millis());

    // Retrieve the current time.
    let time = clock.system_time();

    // Convert the time to various formats.
    let iso: Iso8601 = time.try_into()?;
    let rfc: Rfc2822 = time.try_into()?;

    // Print the current time in various formats.
    println!("Current time (ISO 8601): {iso}");
    println!("Current time (RFC 2822): {rfc}");

    // Calculate the duration between two times.
    clock.delay(Duration::from_secs(1)).await;

    let new_time = clock.system_time();

    // Checked duration returns an error if the second time precedes the first.
    let diff = new_time.duration_since(time)?;

    // Print the difference in milliseconds.
    println!("Time difference: {}ms", diff.as_millis());

    Ok(())
}
