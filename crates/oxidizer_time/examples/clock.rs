// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// This example demonstrates basic interaction with the Clock type.

use std::error::Error;
use std::time::Duration;

use futures::StreamExt;
use oxidizer_time::{Clock, Delay, FutureExt, PeriodicTimer, Stopwatch};

async fn clock_example(clock: Clock) -> Result<(), Box<dyn Error + 'static>> {
    // Use the clock in your APIs. We recommend passing it into the constructor of your API.
    // When the API requires the clock, you can clone it. This preserves the internal link between
    // the clocks.
    let api = MyApi::new(clock.clone());

    // Execute some operation that uses the clock.
    api.do_something().await;

    // Execute periodic timer
    let timer = PeriodicTimer::with_clock(&clock, Duration::from_secs(2));
    timer
        .take(3)
        .for_each(async |()| {
            println!("Timer fired in background!");
        })
        .await;

    // Apply timeout to long operations
    match Delay::with_clock(&clock, Duration::from_secs(30))
        .timeout_with_clock(Duration::from_secs(2), &clock)
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
        let watch = Stopwatch::with_clock(&self.clock);

        // Use the delay.
        Delay::with_clock(&self.clock, Duration::from_millis(10)).await;

        println!(
            "Work done. Elapsed: {}ms, Timestamp: {}",
            watch.elapsed().as_millis(),
            self.clock.now() // Retrieve current time using the clock.
        );
    }
}

#[path = "utils/mini_runtime.rs"]
mod runtime;

fn main() {
    runtime::MiniRuntime::execute(clock_example).unwrap();
}