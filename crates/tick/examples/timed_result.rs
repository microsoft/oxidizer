// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to measure the execution time of an async
//! operation using [`Clock::timed`] and [`TimedResult`].

use tick::{Clock, TimedResult};

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

    // Print the result and the elapsed time.
    println!("Result: {}, Elapsed time: {:?}", result, duration);
}
