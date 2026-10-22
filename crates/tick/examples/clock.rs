// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example shows advanced use of the `Clock` API, including
//! `Stopwatch`, `PeriodicTimer`, and timeouts.

use std::time::Duration;

use tick::fmt::Iso8601;
use tick::{Clock, FutureExt, PeriodicTimer};

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create a clock for the Tokio runtime.
    let clock = Clock::new_tokio();

    // Pass the clock to API constructors. Clone it when an API stores the clock.
    // The clones share the same clock state.
    let api = MyApi::new(&clock);

    // Run an operation that uses the clock.
    api.do_something().await?;

    // Wait for three timer ticks.
    let mut timer = PeriodicTimer::new(&clock, Duration::from_secs(2));
    for _ in 0..3 {
        let tick = timer.next().await;
        println!("Timer fired at {:?}!", tick.observed_at());
    }

    // Apply a timeout to a long operation.
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

    pub(crate) async fn do_something(&self) -> tick::Result<()> {
        // Start measuring elapsed time.
        let watch = self.clock.stopwatch();

        // Use a delay to simulate work.
        self.clock.delay(Duration::from_millis(10)).await;

        println!(
            "Work done. Elapsed: {}ms, Timestamp: {}",
            watch.elapsed().as_millis(),
            self.clock.system_time_as::<Iso8601>()
        );

        Ok(())
    }
}
