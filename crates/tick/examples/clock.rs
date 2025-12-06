// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates basic interaction with the Clock API.

use std::time::Duration;

use futures::StreamExt;
use tick::{Clock, Delay, FutureExt, PeriodicTimer, Stopwatch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Use the clock in your APIs. We recommend passing it into the constructor of your API.
    // When the API requires the clock, you can clone it. This preserves the internal link
    // between the clocks.
    let api = MyApi::new(clock.clone());

    // Execute some operation that uses the clock.
    api.do_something().await;

    // Execute a periodic timer.
    let timer = PeriodicTimer::new(&clock, Duration::from_secs(2));
    timer
        .take(3)
        .for_each(async |()| {
            println!("Timer fired in background!");
        })
        .await;

    // Apply a timeout to long operations.
    match Delay::new(&clock, Duration::from_secs(30))
        .timeout(Duration::from_secs(2), &clock)
        .await
    {
        Ok(()) => println!("Background job completed within the timeout."),
        Err(error) => println!("Background job timed out. Error: {error}"),
    }

    Ok(())
}

struct MyApi {
    clock: Clock,
}

impl MyApi {
    const fn new(clock: Clock) -> Self {
        Self { clock }
    }

    pub async fn do_something(&self) {
        // Start the measurement.
        let watch = Stopwatch::new(&self.clock);

        // Simulate some work with a delay.
        Delay::new(&self.clock, Duration::from_millis(10)).await;

        println!(
            "Work done. Elapsed: {}ms, Timestamp: {}",
            watch.elapsed().as_millis(),
            self.clock.timestamp() // Retrieve the current time using the clock.
        );
    }
}
