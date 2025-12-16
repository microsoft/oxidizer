// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates advanced usage of the `Clock` API, including
//! `Stopwatch`, `PeriodicTimer`, and timeouts.

use std::time::Duration;

use futures::StreamExt;
use tick::{Clock, FutureExt, PeriodicTimer, fmt::Iso8601};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Use the clock in your APIs. We recommend passing it into the constructor of your API.
    // When the API requires the clock, you can clone it. This preserves the internal link
    // between the clocks.
    let api = MyApi::new(&clock);

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
    match clock.delay(Duration::from_secs(30)).timeout(&clock, Duration::from_secs(2)).await {
        Ok(()) => println!("Background job completed within the timeout."),
        Err(error) => println!("Background job timed out. Error: {error}"),
    }

    Ok(())
}

struct MyApi {
    clock: Clock,
}

impl MyApi {
    fn new(clock: &Clock) -> Self {
        Self { clock: clock.clone() }
    }

    pub async fn do_something(&self) {
        // Start the measurement.
        let watch = self.clock.stopwatch();

        // Simulate some work with a delay.
        self.clock.delay(Duration::from_millis(10)).await;

        println!(
            "Work done. Elapsed: {}ms, Timestamp: {}",
            watch.elapsed().as_millis(),
            self.clock.system_time_as::<Iso8601>()
        );
    }
}
