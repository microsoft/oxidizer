// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates advanced usage of the `Clock` API, including
//! `Stopwatch`, `PeriodicTimer`, and timeouts.

use tick::Clock;
use tick::TimedResult;

#[tokio::main]
async fn main() {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Start some background work that returns a result after a delay.
    let background_job = async {
        clock.delay(std::time::Duration::from_millis(10)).await;
        "Background job result"
    };

    // Use `Timed` to measure the time taken by the background job and capture its result.
    let TimedResult { result, duration } = clock.timed(background_job).await;

    // Stop the measurement and print the elapsed time.
    println!("Result: {}, Elapsed time: {:?}", result, duration);
}
