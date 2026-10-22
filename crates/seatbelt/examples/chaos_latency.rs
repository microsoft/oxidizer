// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example adds an artificial delay at a configured rate.
//!
//! It delays 50% of requests by 200 milliseconds.
//! This tests how the service responds to degraded performance.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::chaos::latency::Latency;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define a stack with a latency layer.
    let stack = (
        Latency::layer("my_latency", &context)
            // Set the latency injection rate to 50%.
            .rate(0.5)
            // Set the delay to inject.
            .latency(Duration::from_millis(200)),
        Execute::new(execute_operation),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    for i in 0..10 {
        let stopwatch = clock.stopwatch();
        let result = service.execute(format!("request-{i}")).await;
        let elapsed = stopwatch.elapsed();
        println!("{i}: result = '{result}' (took {elapsed:?})");
    }
}

async fn execute_operation(input: String) -> String {
    format!("processed:{input}")
}
