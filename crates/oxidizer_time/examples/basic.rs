// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This example demonstrates the basic usage of the Oxidizer time APIs.

use std::error::Error;
use std::time::Duration;

use oxidizer_time::fmt::{Iso8601Timestamp, Rfc2822Timestamp};
use oxidizer_time::{Clock, Delay, Stopwatch};

async fn basic(clock: Clock) -> Result<(), Box<dyn Error>> {
    // Use stopwatch to measure elapsed time.
    let stopwatch = Stopwatch::with_clock(&clock);

    // Delay for 2 seconds.
    Delay::with_clock(&clock, Duration::from_secs(2)).await;
    println!("Elapsed time: {}ms", stopwatch.elapsed().as_millis());

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

#[path = "utils/mini_runtime.rs"]
mod runtime;

fn main() {
    runtime::MiniRuntime::execute(basic).unwrap();
}