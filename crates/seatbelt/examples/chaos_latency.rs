// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Chaos latency middleware example that injects artificial delay with a configurable probability.
//!
//! This example simulates a service where 50% of requests are delayed by 200ms,
//! demonstrating how chaos latency can be used to test resilience under degraded
//! performance conditions.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::chaos::latency::Latency;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define stack with latency injection layer
    let stack = (
        Latency::layer("my_latency", &context)
            // Required: probability of latency injection (50%)
            .rate(0.5)
            // Required: fixed delay duration to inject
            .latency(Duration::from_millis(200)),
        Execute::new(execute_operation),
    );

    // Create the service from the stack
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
